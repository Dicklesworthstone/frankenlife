//! Reproduction proposals and the birth transaction (plan §7.4, §7.5,
//! §8.1 phases 7 and 8, §8.2, INV-05, INV-06, A.3 item 10, Appendix C.5).
//!
//! **Phase 7.** [`Reproduction::propose`] reads the post-emission budget and
//! changes nothing:
//! - A dead parent, one whose budget is exhausted, cannot reproduce.
//! - A parent is eligible only once the attempt tick `n`, the committed
//!   input tick of the advance, has reached its cooldown deadline.
//! - An eligible parent makes one Bernoulli attempt per tick. The
//!   probability is `p = 1 − e^(−λΔt)`, decoded by `flife_neural::decode`.
//!   The draw is word 0 of `(birth, parent, n, lane (attempt, 0), 0)`, so
//!   it depends on the parent's stable identity and the tick only.
//! - An attempt must pass the minimum attempt energy, checked against the
//!   post-emission budget `E`. It must then afford the birth cost `c_b`
//!   and leave both a viable child and a viable parent. The split is
//!   `R = E − c_b`, rounded down, the child's endowment `β R`, rounded to
//!   nearest, and the parent's share `R − β R`, rounded down. So
//!   `child + parent ≤ E − c_b` holds exactly, and both minimums are
//!   checked on the values that will be committed. C.5: `E = 10`, `c_b = 1`
//!   and `β = 1/3` give exactly 3 and 6, with 1 dissipated.
//! - A failed attempt never resets the cooldown.
//!
//! **Phase 8.** [`resolve_births`] stages the tick's viable attempts as one
//! transaction, in ascending parent identity whatever order they arrive
//! in. For each:
//! 1. One live-population slot is reserved against the fixed logical
//!    population quota. This happens before any genome work, so the quota
//!    is enforced early.
//! 2. The mutated child is constructed and validated by
//!    `flife_genome::Mutator::propose` (under
//!    `field-ecology-v1-fresh-offspring`, FL-041, the child's genome is
//!    instead a founder's drawn at its own identity, [`Offspring::Fresh`]). The fixed logical genome quota is
//!    charged before any parameter buffer is allocated. A birth whose
//!    candidates are all rejected is itself rejected and charges nothing
//!    (`reject-birth-no-charge`). It takes no slot and no identity.
//! 3. The child receives the next organism and genome identities, in that
//!    order. Its position is the parent's post-motion position plus an
//!    offset uniform by area in the configured disk, `R√u₀` at angle
//!    `2πu₁`, from words 0 and 1 of `(birth, parent, n, lane (position, 0),
//!    0)`, wrapped onto the torus.
//!
//! When either fixed logical quota cannot cover a birth, under
//! `pause-before-commit` the whole stage is rolled back and reported as a
//! [`CapacityShortfall`]. Every staged genome's charge is returned, and no
//! identity, slot or energy is committed. Retrying the unchanged candidate
//! reproduces the same proposals, randomness and identities. Any other
//! failure, such as a physical allocation failure, also rolls everything
//! back. It is returned as an error that pauses, retries or fails the run,
//! and never becomes a birth-selection rule (§7.5).
//!
//! **Commit.** A [`Birth`] carries the child's identity, genome, slot, and
//! energy together (INV-06). [`Births::settle`] checks every parent's budget
//! against its proposal before changing any, then returns the budgets the
//! parents keep and records one `child-endowment` transfer per birth. Each
//! transfer debits `E − parent`, credits the child, and charges the
//! difference, the birth cost, to `reproduction-loss`. [`Births::abandon`]
//! returns every staged charge when the candidate tick is discarded (§8.3).
//!
//! Newborns (`next-tick`) first exist in the committed state of tick
//! `n + 1`, which is their birth tick. They start with zero velocity, zero
//! energy-change history, an invalid held controller output that forces
//! their first evaluation, and their own stable phase. They inherit their
//! parent's lineage at the next generation. A birth sets both the child's
//! and the parent's cooldown deadline to `n + 1 + ⌈cooldown/Δt⌉`.

use flife_core::clock::{Tick, TickSpan, stable_phase};
use flife_core::ids::{Generation, GenomeId, IdSequence, LineageId, OrganismId};
use flife_core::ledger::{EnergyBudget, TickLedger, Transfer, sub_down};
use flife_core::numerics::sin_cos_turns;
use flife_core::rng::{
    Address, Block, Lane, LaneKind, SemanticRng, Stream, bernoulli, half_open_unit,
};
use flife_core::{CoreError, ValidatedConfig};
use flife_field::FieldGrid;
use flife_genome::{
    BirthAttempt, CandidateRejection, Genome, GenomeQuota, LOGICAL_GENOME_BYTES, MutationRecord,
    Mutator, Proposal, TopologyCatalog, TraitLimits, founder,
};
use flife_neural::DecodedActions;

/// The newborn activation this module implements: newborns first take part
/// on the tick after their birth.
pub const NEXT_TICK_ACTIVATION: &str = "next-tick";

/// The constants of `[reproduction]`, `costs.birth_energy` and the
/// controller interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReproductionRules {
    /// `c_b`, dissipated by every birth.
    pub birth_cost: f64,
    /// The minimum post-emission energy for an attempt.
    pub minimum_attempt_energy: f64,
    /// The minimum child endowment, `> 0`.
    pub minimum_child_energy: f64,
    /// The minimum energy the parent keeps, `> 0`.
    pub minimum_parent_energy: f64,
    /// `⌈cooldown_seconds / Δt⌉`.
    pub cooldown: TickSpan,
    /// The child offset disk's radius, in world units.
    pub offset_radius: f64,
    /// The controller evaluation interval, for the newborn's stable phase.
    pub controller_interval: u32,
}

/// The validated birth rules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reproduction {
    rules: ReproductionRules,
}

impl Reproduction {
    /// Validated rules: a finite nonnegative birth cost, attempt minimum
    /// and offset radius, strictly positive child and parent minimums, and
    /// an interval of at least one tick.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a value out of range.
    pub fn new(rules: ReproductionRules) -> Result<Self, CoreError> {
        for (quantity, value, positive) in [
            ("costs.birth_energy", rules.birth_cost, false),
            (
                "reproduction.minimum_parent_energy_before_attempt",
                rules.minimum_attempt_energy,
                false,
            ),
            (
                "reproduction.minimum_child_energy",
                rules.minimum_child_energy,
                true,
            ),
            (
                "reproduction.minimum_parent_energy_after_split",
                rules.minimum_parent_energy,
                true,
            ),
            (
                "reproduction.child_position_offset_radius",
                rules.offset_radius,
                false,
            ),
        ] {
            let admissible = value.is_finite() && if positive { value > 0.0 } else { value >= 0.0 };
            if !admissible {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{value} is out of range"),
                });
            }
        }
        if rules.controller_interval == 0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.evaluation_interval_ticks",
                detail: "0 ticks; the interval must be at least 1".into(),
            });
        }
        Ok(Self { rules })
    }

    /// The rules of a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another newborn activation;
    /// otherwise as [`Reproduction::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let model = config.config();
        let reproduction = &model.reproduction;
        if reproduction.newborn_activation != NEXT_TICK_ACTIVATION {
            return Err(CoreError::UnknownAlgorithm {
                role: "reproduction.newborn_activation",
                name: reproduction.newborn_activation.clone(),
            });
        }
        Self::new(ReproductionRules {
            birth_cost: model.costs.birth_energy,
            minimum_attempt_energy: reproduction.minimum_parent_energy_before_attempt,
            minimum_child_energy: reproduction.minimum_child_energy,
            minimum_parent_energy: reproduction.minimum_parent_energy_after_split,
            cooldown: config.report().cooldown_ticks,
            offset_radius: reproduction.child_position_offset_radius,
            controller_interval: model.controller.evaluation_interval_ticks,
        })
    }

    /// The rules.
    #[must_use]
    pub const fn rules(&self) -> &ReproductionRules {
        &self.rules
    }

    /// Whether `parent` attempts a birth at tick `tick` with probability
    /// `probability`: word 0 of `(birth, parent, tick, lane (attempt, 0),
    /// 0)` against `p`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `0 ≤ p ≤ 1`.
    pub fn attempts(
        &self,
        rng: &SemanticRng,
        parent: OrganismId,
        tick: Tick,
        probability: f64,
    ) -> Result<bool, CoreError> {
        if !(0.0..=1.0).contains(&probability) {
            return Err(CoreError::NumericalInvalid {
                quantity: "reproduction attempt probability",
                detail: format!("{probability} is outside [0, 1]"),
            });
        }
        let [word, ..] = birth_block(rng, parent, tick, LaneKind::Attempt)?;
        Ok(bernoulli(word, probability))
    }

    /// The child's offset from its parent, uniform by area in the disk of
    /// radius `R`: `R√u₀` at angle `2πu₁`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] if a draw leaves the unit interval,
    /// which does not happen.
    pub fn child_offset(
        &self,
        rng: &SemanticRng,
        parent: OrganismId,
        tick: Tick,
    ) -> Result<[f64; 2], CoreError> {
        let [first, second, ..] = birth_block(rng, parent, tick, LaneKind::Position)?;
        let radius = self.rules.offset_radius * half_open_unit(first).sqrt();
        let (sin, cos) =
            sin_cos_turns(half_open_unit(second)).ok_or_else(|| CoreError::NumericalInvalid {
                quantity: "child offset angle",
                detail: format!("word {second:#018x} left the unit interval"),
            })?;
        Ok([radius * cos, radius * sin])
    }

    /// The split of `available`, the post-emission budget, at child share
    /// `β`, or why no viable birth fits it.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `available` is finite and
    /// nonnegative and `0 < β < 1`.
    pub fn split(&self, available: f64, child_fraction: f64) -> Result<Viability, CoreError> {
        if !(available.is_finite() && available >= 0.0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "parent energy",
                detail: format!("{available} is not a finite nonnegative energy"),
            });
        }
        if !(child_fraction > 0.0 && child_fraction < 1.0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "child fraction",
                detail: format!("{child_fraction} is outside (0, 1)"),
            });
        }
        let rules = &self.rules;
        if available < rules.minimum_attempt_energy {
            return Ok(Viability::Unviable(Unviable::BelowAttemptEnergy {
                available,
                minimum: rules.minimum_attempt_energy,
            }));
        }
        let remainder = sub_down(available, rules.birth_cost);
        if remainder <= 0.0 {
            return Ok(Viability::Unviable(Unviable::BirthCostUnaffordable {
                available,
                cost: rules.birth_cost,
            }));
        }
        // `β < 1`, so `β R ≤ R` after rounding and the parent's share is
        // nonnegative.
        let child = child_fraction * remainder;
        let parent = sub_down(remainder, child);
        if child < rules.minimum_child_energy {
            return Ok(Viability::Unviable(Unviable::ChildBelowMinimum {
                child,
                minimum: rules.minimum_child_energy,
            }));
        }
        if parent < rules.minimum_parent_energy {
            return Ok(Viability::Unviable(Unviable::ParentBelowMinimum {
                parent,
                minimum: rules.minimum_parent_energy,
            }));
        }
        Ok(Viability::Viable(Split {
            available: available + 0.0,
            child,
            parent,
        }))
    }

    /// Phase 7's reproduction proposal for `parent`, read from its
    /// post-emission `budget` and this tick's decoded `actions`, at attempt
    /// tick `tick`. Nothing changes.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for an invalid decoded probability or
    /// share, and [`CoreError::Overflow`] for a child generation past
    /// `u32::MAX`.
    pub fn propose(
        &self,
        rng: &SemanticRng,
        parent: &ParentState,
        tick: Tick,
        actions: &DecodedActions,
        budget: &EnergyBudget,
    ) -> Result<ReproductionProposal, CoreError> {
        if budget.is_exhausted() {
            return Ok(ReproductionProposal::Exhausted);
        }
        if tick < parent.cooldown_deadline {
            return Ok(ReproductionProposal::CoolingDown {
                deadline: parent.cooldown_deadline,
            });
        }
        if !self.attempts(rng, parent.id, tick, actions.attempt_probability)? {
            return Ok(ReproductionProposal::NoAttempt);
        }
        match self.split(budget.remaining(), actions.child_fraction)? {
            Viability::Unviable(reason) => Ok(ReproductionProposal::Unviable(reason)),
            Viability::Viable(split) => Ok(ReproductionProposal::Intent(BirthIntent {
                parent: parent.id,
                lineage: parent.lineage,
                generation: parent.generation.child()?,
                tick,
                position: parent.position,
                split,
            })),
        }
    }
}

fn birth_block(
    rng: &SemanticRng,
    parent: OrganismId,
    tick: Tick,
    kind: LaneKind,
) -> Result<Block, CoreError> {
    Ok(rng.block(Address {
        stream: Stream::Birth,
        entity: parent.get(),
        ordinal: tick.get(),
        lane: Lane::new(0, kind, 0)?,
        draw: 0,
    }))
}

/// What phase 7 reads of a parent besides its budget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParentState {
    /// The parent's stable identity.
    pub id: OrganismId,
    /// Its founder lineage.
    pub lineage: LineageId,
    /// Its generation.
    pub generation: Generation,
    /// Its post-motion position.
    pub position: [f64; 2],
    /// The first attempt tick at which it is eligible.
    pub cooldown_deadline: Tick,
}

/// An energy split that leaves a viable child and parent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Split {
    /// `E`, the parent's post-emission budget.
    pub available: f64,
    /// The child's endowment, `β (E − c_b)`.
    pub child: f64,
    /// What the parent keeps, `(1 − β)(E − c_b)`.
    pub parent: f64,
}

impl Split {
    /// What the parent pays, `E − parent`, rounded to nearest: the child's
    /// endowment plus the birth cost.
    #[must_use]
    pub fn debited(&self) -> f64 {
        self.available - self.parent
    }

    /// What the birth dissipates, `E − parent − child`: the birth cost, plus
    /// at most a few units in the last place of the split's rounding.
    #[must_use]
    pub fn dissipated(&self) -> f64 {
        self.debited() - self.child
    }
}

/// Whether a split is viable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Viability {
    /// Both shares meet their minimums.
    Viable(Split),
    /// No viable birth fits the budget.
    Unviable(Unviable),
}

/// Why an attempted birth is not viable (§7.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Unviable {
    /// The post-emission budget is below the attempt minimum.
    BelowAttemptEnergy {
        /// The budget.
        available: f64,
        /// The minimum.
        minimum: f64,
    },
    /// Nothing is left after the birth cost.
    BirthCostUnaffordable {
        /// The budget.
        available: f64,
        /// The birth cost.
        cost: f64,
    },
    /// The child's endowment is below its minimum.
    ChildBelowMinimum {
        /// The endowment.
        child: f64,
        /// The minimum.
        minimum: f64,
    },
    /// The parent's share is below its minimum.
    ParentBelowMinimum {
        /// The share.
        parent: f64,
        /// The minimum.
        minimum: f64,
    },
}

/// Phase 7's reproduction proposal for one organism.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReproductionProposal {
    /// The organism is dead.
    Exhausted,
    /// The attempt tick is before its cooldown deadline.
    CoolingDown {
        /// The deadline.
        deadline: Tick,
    },
    /// Eligible, but this tick's draw made no attempt.
    NoAttempt,
    /// Attempted, but no viable birth fits the post-emission budget.
    Unviable(Unviable),
    /// A viable attempt for phase 8.
    Intent(BirthIntent),
}

/// A viable attempt: what phase 8 needs to construct the birth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BirthIntent {
    /// The parent.
    pub parent: OrganismId,
    /// The parent's lineage, which the child inherits.
    pub lineage: LineageId,
    /// The child's generation.
    pub generation: Generation,
    /// The attempt tick `n`, the advance's committed input tick.
    pub tick: Tick,
    /// The parent's post-motion position.
    pub position: [f64; 2],
    /// The energy split.
    pub split: Split,
}

/// One birth for phase 8: an intent and its parent's genome.
#[derive(Debug, Clone, Copy)]
pub struct BirthRequest<'g> {
    /// The viable attempt.
    pub intent: BirthIntent,
    /// The parent's genome.
    pub genome: &'g Genome,
}

/// The fixed logical live-population quota for one tick's births (§7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopulationQuota {
    maximum: u64,
    live: u64,
}

impl PopulationQuota {
    /// `live` organisms will be live at publication before this tick's
    /// births, of at most `maximum`.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when `live > maximum`.
    pub fn new(maximum: u64, live: u64) -> Result<Self, CoreError> {
        if live > maximum {
            return Err(CoreError::StateConflict {
                operation: "population quota",
                detail: format!("{live} live organisms exceed the quota {maximum}"),
            });
        }
        Ok(Self { maximum, live })
    }

    /// `population.maximum_count`, with `live` organisms live.
    ///
    /// # Errors
    /// As [`PopulationQuota::new`].
    pub fn from_config(config: &ValidatedConfig, live: u64) -> Result<Self, CoreError> {
        Self::new(u64::from(config.config().population.maximum_count), live)
    }

    /// Slots this tick's births may fill.
    #[must_use]
    pub const fn available(&self) -> u64 {
        self.maximum - self.live
    }
}

/// What phase 8 constructs from, besides the requests.
#[derive(Debug, Clone, Copy)]
pub struct BirthContext<'a> {
    /// The world's semantic randomness.
    pub rng: &'a SemanticRng,
    /// The torus children are placed on.
    pub grid: &'a FieldGrid,
    /// The birth rules.
    pub reproduction: &'a Reproduction,
    /// The mutation operator.
    pub mutator: &'a Mutator<'a>,
    /// Where a newborn's genome comes from.
    pub offspring: Offspring<'a>,
}

/// Where a newborn's genome comes from.
#[derive(Debug, Clone, Copy)]
pub enum Offspring<'a> {
    /// Its parent's genome through [`Mutator::propose`]
    /// (`field-ecology-v1`).
    Inherited,
    /// A founder's genome drawn at the newborn's own organism identity
    /// (`field-ecology-v1-fresh-offspring`, FL-041): the default topology,
    /// founder weights and zero biases, founder traits. Nothing of the
    /// parent's genome passes to it, and no mutation is proposed.
    Fresh {
        /// The topology catalog.
        catalog: &'a TopologyCatalog,
        /// The trait limits.
        limits: &'a TraitLimits,
        /// The configured initial sensing scale.
        initial_scale: f64,
    },
}

/// The run's identity sequences: the next organism and genome identities
/// to issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identities {
    /// Organism identities.
    pub organisms: IdSequence<OrganismId>,
    /// Genome identities.
    pub genomes: IdSequence<GenomeId>,
}

/// A fixed logical quota a birth needs (§7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capacity {
    /// `population.maximum_count`.
    PopulationSlots,
    /// `execution.maximum_logical_genome_bytes`.
    LogicalGenomeBytes,
}

/// A birth the fixed logical quotas cannot cover. Under
/// `pause-before-commit` the tick pauses before commit with nothing staged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityShortfall {
    /// The quota.
    pub resource: Capacity,
    /// The parent of the first birth it could not cover.
    pub parent: OrganismId,
    /// What that birth needed: one slot, or its genome's logical bytes.
    pub required: u64,
    /// What was left.
    pub available: u64,
    /// Every proposed birth's parent, in the stable order they were staged.
    pub proposed: Vec<OrganismId>,
    /// Births covered before the refused one.
    pub covered: usize,
}

/// A birth whose every candidate was rejected: no child, no charge, and the
/// parent's energy and cooldown unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedBirth {
    /// The parent.
    pub parent: OrganismId,
    /// Every candidate's rejection, in order.
    pub candidates: Vec<CandidateRejection>,
}

/// A newborn's initial state (§6.6, §7.4, A.3 item 10).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Newborn {
    /// Its stable identity.
    pub id: OrganismId,
    /// Its genome's identity.
    pub genome: GenomeId,
    /// Its parent.
    pub parent: OrganismId,
    /// Its founder lineage, inherited.
    pub lineage: LineageId,
    /// Its generation, one past its parent's.
    pub generation: Generation,
    /// `n + 1`: the first committed tick it exists in.
    pub birth_tick: Tick,
    /// Its wrapped position.
    pub position: [f64; 2],
    /// Its offset from the parent's post-motion position.
    pub offset: [f64; 2],
    /// Zero.
    pub velocity: [f64; 2],
    /// Its endowment.
    pub energy: f64,
    /// Zero: newborns start with no energy-change history.
    pub energy_change_rate: f64,
    /// False: its first advance forces a controller evaluation.
    pub held_output_valid: bool,
    /// Its stable controller phase, identity modulo the interval.
    pub phase: u32,
    /// `n + 1 + ⌈cooldown/Δt⌉`.
    pub cooldown_deadline: Tick,
}

/// One staged birth: identity, genome, slot and energy together (INV-06).
#[derive(Debug)]
pub struct Birth {
    /// The parent.
    pub parent: OrganismId,
    /// The energy split.
    pub split: Split,
    /// The parent's new cooldown deadline, the same as the child's.
    pub parent_cooldown_deadline: Tick,
    /// The child.
    pub newborn: Newborn,
    /// The child's sealed genome, charged to the genome quota.
    pub genome: Box<Genome>,
    /// How the genome was made.
    pub record: MutationRecord,
}

impl Birth {
    /// What the parent keeps: `budget` with exactly the split's parent
    /// share.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] unless `budget` still holds exactly the
    /// energy the split was proposed against.
    pub fn settle_parent(&self, budget: &EnergyBudget) -> Result<EnergyBudget, CoreError> {
        if budget.is_exhausted() || budget.remaining().to_bits() != self.split.available.to_bits() {
            return Err(CoreError::StateConflict {
                operation: "settle birth",
                detail: format!(
                    "{} holds {}, but its birth was proposed against {}",
                    self.parent,
                    budget.remaining(),
                    self.split.available
                ),
            });
        }
        let mut kept = *budget;
        kept.retain(self.split.parent)?;
        Ok(kept)
    }
}

/// A tick's staged births.
#[derive(Debug)]
pub struct Births {
    born: Vec<Birth>,
    rejected: Vec<RejectedBirth>,
}

impl Births {
    /// The staged births, in ascending parent identity, which is also
    /// ascending child identity.
    #[must_use]
    pub fn born(&self) -> &[Birth] {
        &self.born
    }

    /// Births whose every candidate was rejected.
    #[must_use]
    pub fn rejected(&self) -> &[RejectedBirth] {
        &self.rejected
    }

    /// Settle every parent and record the endowments, all or nothing.
    /// `budget_of` gives each parent's budget at the reproduction stage.
    /// Returns, in birth order, the budget each parent keeps, for the
    /// caller to commit with the newborns.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when a parent is missing or its budget
    /// no longer matches its proposal. Nothing is recorded on error.
    pub fn settle(
        &self,
        ledger: &mut TickLedger,
        budget_of: impl Fn(OrganismId) -> Option<EnergyBudget>,
    ) -> Result<Vec<(OrganismId, EnergyBudget)>, CoreError> {
        let mut kept = Vec::with_capacity(self.born.len());
        for birth in &self.born {
            let budget = budget_of(birth.parent).ok_or_else(|| CoreError::StateConflict {
                operation: "settle birth",
                detail: format!("{} has no budget", birth.parent),
            })?;
            kept.push((birth.parent, birth.settle_parent(&budget)?));
        }
        for birth in &self.born {
            ledger.transfer(
                Transfer::ChildEndowment,
                birth.split.debited(),
                birth.split.child,
            )?;
        }
        Ok(kept)
    }

    /// Discard the staged births, returning every genome's charge, as when
    /// the candidate tick is cancelled before publication (§8.3).
    ///
    /// # Errors
    /// As [`GenomeQuota::retire`].
    pub fn abandon(self, quota: &mut GenomeQuota) -> Result<(), CoreError> {
        roll_back(self.born, quota)
    }

    /// The births and the rejected births.
    #[must_use]
    pub fn into_parts(self) -> (Vec<Birth>, Vec<RejectedBirth>) {
        (self.born, self.rejected)
    }
}

/// The outcome of phase 8's birth stage.
#[derive(Debug)]
pub enum BirthResolution {
    /// Every viable attempt was staged or rejected; the identities advanced.
    Staged(Births),
    /// A fixed logical quota could not cover a birth. Nothing is staged or
    /// charged and the identities are unchanged.
    Paused(CapacityShortfall),
}

/// Stage `requests`, the tick's viable attempts, as one transaction (see the
/// [module documentation](self)).
///
/// # Errors
/// [`CoreError::StateConflict`] for two requests from one parent or for
/// requests of different ticks, [`CoreError::Overflow`] for an exhausted
/// identity sequence or tick, and any error of mutation other than a
/// logical genome quota refusal, such as a physical allocation failure.
/// Everything is rolled back first: no genome stays charged and the
/// identities are unchanged.
pub fn resolve_births(
    context: &BirthContext<'_>,
    requests: &[BirthRequest<'_>],
    population: PopulationQuota,
    identities: &mut Identities,
    quota: &mut GenomeQuota,
) -> Result<BirthResolution, CoreError> {
    let mut order: Vec<&BirthRequest<'_>> = requests.iter().collect();
    order.sort_unstable_by_key(|request| request.intent.parent);
    if let Some(pair) = order
        .windows(2)
        .find(|pair| pair[0].intent.parent == pair[1].intent.parent)
    {
        return Err(CoreError::StateConflict {
            operation: "resolve births",
            detail: format!("{} proposed two births", pair[0].intent.parent),
        });
    }
    let Some(tick) = order.first().map(|request| request.intent.tick) else {
        return Ok(BirthResolution::Staged(Births {
            born: Vec::new(),
            rejected: Vec::new(),
        }));
    };
    if order.iter().any(|request| request.intent.tick != tick) {
        return Err(CoreError::StateConflict {
            operation: "resolve births",
            detail: "the requests belong to different ticks".into(),
        });
    }
    let birth_tick = tick.next()?;
    let deadline = birth_tick.checked_add(context.reproduction.rules.cooldown)?;

    let mut working = identities.clone();
    // Each request ends in one birth or one rejected birth: room for every
    // request in both lists, so neither grows (FL-043).
    let mut born: Vec<Birth> = Vec::new();
    let mut rejected: Vec<RejectedBirth> = Vec::new();
    reserve_births(&mut born, order.len(), "births")?;
    reserve_births(&mut rejected, order.len(), "rejected births")?;
    for request in &order {
        match stage(
            context,
            request,
            birth_tick,
            deadline,
            population,
            &mut working,
            quota,
            born.len(),
        ) {
            Ok(Staged::Born(birth)) => born.push(*birth),
            Ok(Staged::Rejected(birth)) => rejected.push(birth),
            Ok(Staged::Short {
                resource,
                required,
                available,
            }) => {
                let covered = born.len();
                roll_back(born, quota)?;
                return Ok(BirthResolution::Paused(CapacityShortfall {
                    resource,
                    parent: request.intent.parent,
                    required,
                    available,
                    proposed: order.iter().map(|request| request.intent.parent).collect(),
                    covered,
                }));
            }
            Err(error) => {
                roll_back(born, quota)?;
                return Err(error);
            }
        }
    }
    *identities = working;
    Ok(BirthResolution::Staged(Births { born, rejected }))
}

/// One request's staging outcome.
enum Staged {
    Born(Box<Birth>),
    Rejected(RejectedBirth),
    Short {
        resource: Capacity,
        required: u64,
        available: u64,
    },
}

#[expect(
    clippy::too_many_arguments,
    reason = "private staging step of resolve_births; the arguments are its working state"
)]
fn stage(
    context: &BirthContext<'_>,
    request: &BirthRequest<'_>,
    birth_tick: Tick,
    deadline: Tick,
    population: PopulationQuota,
    identities: &mut Identities,
    quota: &mut GenomeQuota,
    staged: usize,
) -> Result<Staged, CoreError> {
    let intent = &request.intent;
    let staged = u64::try_from(staged).unwrap_or(u64::MAX);
    if staged >= population.available() {
        return Ok(Staged::Short {
            resource: Capacity::PopulationSlots,
            required: 1,
            available: population.available().saturating_sub(staged),
        });
    }
    let offset = context
        .reproduction
        .child_offset(context.rng, intent.parent, intent.tick)?;
    let position = context.grid.wrap_position([
        intent.position[0] + offset[0],
        intent.position[1] + offset[1],
    ])?;
    let mut issued = identities.clone();
    let child = issued.organisms.issue()?;
    let genome_id = issued.genomes.issue()?;
    let phase = stable_phase(child, context.reproduction.rules.controller_interval)?;
    let attempt = BirthAttempt {
        parent: intent.parent,
        tick: intent.tick,
    };
    let proposed = match context.offspring {
        Offspring::Inherited => context
            .mutator
            .propose(request.genome, attempt, genome_id, quota),
        Offspring::Fresh {
            catalog,
            limits,
            initial_scale,
        } => founder(
            context.rng,
            child,
            genome_id,
            catalog,
            quota,
            limits,
            initial_scale,
        )
        .map(|genome| Proposal::Child {
            genome: Box::new(genome),
            record: MutationRecord {
                candidate: 0,
                topology: None,
                replaced: 0,
                perturbed: 0,
                rejected: Vec::new(),
            },
        }),
    };
    let proposal = match proposed {
        Ok(proposal) => proposal,
        Err(CoreError::BudgetExhausted {
            budget,
            required,
            available,
        }) if budget == LOGICAL_GENOME_BYTES => {
            return Ok(Staged::Short {
                resource: Capacity::LogicalGenomeBytes,
                required,
                available,
            });
        }
        Err(error) => return Err(error),
    };
    match proposal {
        Proposal::Rejected { candidates } => Ok(Staged::Rejected(RejectedBirth {
            parent: intent.parent,
            candidates,
        })),
        Proposal::Child { genome, record } => {
            *identities = issued;
            Ok(Staged::Born(Box::new(Birth {
                parent: intent.parent,
                split: intent.split,
                parent_cooldown_deadline: deadline,
                newborn: Newborn {
                    id: child,
                    genome: genome.id(),
                    parent: intent.parent,
                    lineage: intent.lineage,
                    generation: intent.generation,
                    birth_tick,
                    position,
                    offset,
                    velocity: [0.0, 0.0],
                    energy: intent.split.child,
                    energy_change_rate: 0.0,
                    held_output_valid: false,
                    phase,
                    cooldown_deadline: deadline,
                },
                genome,
                record,
            })))
        }
    }
}

/// Return every staged genome's charge, newest first.
/// Room for exactly `count` more in `list`.
fn reserve_births<T>(
    list: &mut Vec<T>,
    count: usize,
    buffer: &'static str,
) -> Result<(), CoreError> {
    list.try_reserve_exact(count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer,
            bytes: flife_core::memory::bytes_for::<T>(count),
        })
}

fn roll_back(born: Vec<Birth>, quota: &mut GenomeQuota) -> Result<(), CoreError> {
    for birth in born.into_iter().rev() {
        quota.retire(*birth.genome)?;
    }
    Ok(())
}
