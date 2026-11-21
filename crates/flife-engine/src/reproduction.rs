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
//!    instead a founder's drawn at its own identity, [`Offspring::Fresh`]).
//!    Through [`resolve_births_with`], a birth's first candidate can come
//!    with its parameters already mutated elsewhere (FL-032's device), for
//!    `Mutator::propose_prepared`. The fixed logical genome quota is
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
//! `pause-before-commit` (research mode, §7.5) the whole stage is rolled
//! back and reported as a [`CapacityShortfall`]. Every staged genome's
//! charge is returned, and no identity, slot or energy is committed.
//! Retrying the unchanged candidate reproduces the same proposals,
//! randomness and identities.
//!
//! Under `refuse-excess-births-v1`, §7.5's exhibition-mode birth-rejection
//! policy, the same quotas are enforced by refusing births instead:
//! - When the tick's viable attempts outnumber the free population slots,
//!   the slots go to the attempts with the smallest admission keys, word 0
//!   of `(birth, parent, n, lane (attempt, 1), 0)`. The key depends on the
//!   parent's stable identity and the tick only, so the choice is
//!   independent of arrival order, observation and backend, and it does
//!   not favour old (low-identity) parents the way ascending identity
//!   would. Ties, which need equal 64-bit words, fall to the lower
//!   identity.
//! - An admitted birth the logical genome quota cannot cover is refused in
//!   turn, in ascending parent identity, and later births are still tried.
//! - A refused birth ([`RefusedBirth`]) takes no slot, identity or genome
//!   charge. Its parent keeps its energy, and its cooldown is unchanged, as
//!   for any failed attempt, so it may attempt again next tick.
//!
//! The policy is part of the experiment's identity, and every refusal is
//! reported, so the capacity pressure it imposes stays visible (§7.5).
//!
//! Any other failure, such as a physical allocation failure, rolls
//! everything back under either policy. It is returned as an error that
//! pauses, retries or fails the run, and never becomes a birth-selection
//! rule (§7.5).
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

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Barrier, mpsc};

use flife_core::clock::{Tick, TickSpan, stable_phase};
use flife_core::ids::{Generation, GenomeId, IdSequence, LineageId, OrganismId};
use flife_core::ledger::{EnergyBudget, TickLedger, Transfer, sub_down};
use flife_core::numerics::sin_cos_turns;
use flife_core::rng::{
    Address, Block, Lane, LaneKind, SemanticRng, Stream, bernoulli, half_open_unit,
};
use flife_core::{CoreError, ValidatedConfig};
use flife_field::{FieldGrid, ProposalConstants};
use flife_genome::{
    BirthAttempt, CandidateRejection, DetachedProposal, Genome, GenomeQuota, LOGICAL_GENOME_BYTES,
    MutationRecord, Mutator, Prepared, Proposal, TopologyCatalog, TraitLimits, founder,
};
use flife_neural::DecodedActions;

/// Fewest admitted inherited births before spawning parallel proposal workers
/// (`flife-s0t`). Below this, proposing on the main thread avoids sync cost.
const MIN_PARALLEL_BIRTHS: usize = 4;

/// Maximum in-flight detached genome scratch per parallel wave (`flife-s0t`,
/// FL-043): keeps parallel birth proposals strictly within staging memory.
const PARALLEL_PROPOSAL_BUDGET_BYTES: u64 = 2 << 20;

/// Most requests proposed in one parallel wave before committing serially.
const MAX_PARALLEL_WAVE: usize = 128;

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

    /// The constants a settle backend's proposal takes for these rules in a
    /// world of seed `seed` (FL-032).
    #[must_use]
    pub const fn proposal_constants(&self, seed: u64) -> ProposalConstants {
        ProposalConstants {
            seed,
            minimum_attempt_energy: self.rules.minimum_attempt_energy,
            birth_cost: self.rules.birth_cost,
            minimum_child_energy: self.rules.minimum_child_energy,
            minimum_parent_energy: self.rules.minimum_parent_energy,
        }
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
        check_probability(probability)?;
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
        self.attempted(parent, tick, actions.child_fraction, budget)
    }

    /// The proposal of an attempt that the draw made: the split of the
    /// budget at child share `child_fraction`, and the intent a viable one
    /// gives.
    ///
    /// # Errors
    /// As [`Self::split`]; [`CoreError::Overflow`] for a child generation
    /// past `u32::MAX`.
    pub(crate) fn attempted(
        &self,
        parent: &ParentState,
        tick: Tick,
        child_fraction: f64,
        budget: &EnergyBudget,
    ) -> Result<ReproductionProposal, CoreError> {
        match self.split(budget.remaining(), child_fraction)? {
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

/// [`Reproduction::attempts`]' check of an attempt probability.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] unless `0 ≤ p ≤ 1`.
pub(crate) fn check_probability(probability: f64) -> Result<(), CoreError> {
    if (0.0..=1.0).contains(&probability) {
        Ok(())
    } else {
        Err(CoreError::NumericalInvalid {
            quantity: "reproduction attempt probability",
            detail: format!("{probability} is outside [0, 1]"),
        })
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
    /// What a birth the fixed logical quotas cannot cover does.
    pub capacity: CapacityPolicy,
}

/// `population.capacity_policy`: what a birth the fixed logical quotas
/// cannot cover does (§7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityPolicy {
    /// `pause-before-commit`, research mode: the tick pauses before commit
    /// with nothing staged.
    PauseBeforeCommit,
    /// `refuse-excess-births-v1`, exhibition mode: births are admitted in
    /// admission-key order until a quota is full, and the rest are refused
    /// without charge (see the [module documentation](self)).
    RefuseExcessBirths,
}

/// The research-mode capacity policy.
pub const PAUSE_BEFORE_COMMIT: &str = "pause-before-commit";

/// The exhibition-mode capacity policy.
pub const REFUSE_EXCESS_BIRTHS: &str = "refuse-excess-births-v1";

impl CapacityPolicy {
    /// The policy named `name`.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for any other name.
    pub fn from_name(name: &str) -> Result<Self, CoreError> {
        match name {
            PAUSE_BEFORE_COMMIT => Ok(Self::PauseBeforeCommit),
            REFUSE_EXCESS_BIRTHS => Ok(Self::RefuseExcessBirths),
            _ => Err(CoreError::UnknownAlgorithm {
                role: "population.capacity_policy",
                name: name.to_owned(),
            }),
        }
    }

    /// The registered name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PauseBeforeCommit => PAUSE_BEFORE_COMMIT,
            Self::RefuseExcessBirths => REFUSE_EXCESS_BIRTHS,
        }
    }
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

/// A viable attempt `refuse-excess-births-v1` refused because a fixed
/// logical quota could not cover it: no child, no charge, and the parent's
/// energy and cooldown unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusedBirth {
    /// The parent.
    pub parent: OrganismId,
    /// The quota that could not cover it.
    pub resource: Capacity,
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
    refused: Vec<RefusedBirth>,
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

    /// Births `refuse-excess-births-v1` refused, in ascending parent
    /// identity.
    #[must_use]
    pub fn refused(&self) -> &[RefusedBirth] {
        &self.refused
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

    /// The births, the rejected births and the refused births.
    #[must_use]
    pub fn into_parts(self) -> (Vec<Birth>, Vec<RejectedBirth>, Vec<RefusedBirth>) {
        (self.born, self.rejected, self.refused)
    }
}

/// The outcome of phase 8's birth stage.
#[derive(Debug)]
pub enum BirthResolution {
    /// Every viable attempt was staged, rejected or refused; the
    /// identities advanced.
    Staged(Births),
    /// Under `pause-before-commit`, a fixed logical quota could not cover a
    /// birth. Nothing is staged or charged and the identities are
    /// unchanged.
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
    resolve_births_with(context, &[], requests, population, identities, quota)
}

/// [`resolve_births`], where an inherited birth whose parent has an entry
/// in `first` (sorted by parent) proposes its first candidate from it,
/// through `Mutator::propose_prepared`: its member and its parameters
/// already mutated (FL-032).
///
/// # Errors
/// As [`resolve_births`], and as `Mutator::propose_prepared` for a
/// prepared candidate that is not its birth's.
pub fn resolve_births_with(
    context: &BirthContext<'_>,
    first: &[(OrganismId, Prepared<'_>)],
    requests: &[BirthRequest<'_>],
    population: PopulationQuota,
    identities: &mut Identities,
    quota: &mut GenomeQuota,
) -> Result<BirthResolution, CoreError> {
    resolve_births_with_workers(context, first, requests, population, identities, quota, 1)
}

/// Bounded-wave preparer for first-candidate parameter mutations on a settle
/// backend (FL-032, FL-043): mutates at most one wave within
/// [`PARALLEL_PROPOSAL_BUDGET_BYTES`] at a time so host staging stays bounded.
pub(crate) trait FirstCandidateWave {
    fn active(&self) -> bool;
    fn prepare_wave(&mut self, wave: &[&BirthRequest<'_>]);
    fn prepared(&self) -> Vec<(OrganismId, Prepared<'_>)>;
}

struct NoWavePreparer;

impl FirstCandidateWave for NoWavePreparer {
    fn active(&self) -> bool {
        false
    }

    fn prepare_wave(&mut self, _wave: &[&BirthRequest<'_>]) {}

    fn prepared(&self) -> Vec<(OrganismId, Prepared<'_>)> {
        Vec::new()
    }
}

/// [`resolve_births_with`], proposing admitted inherited newborns' genomes in
/// bounded parallel waves across up to `workers` threads before committing
/// them serially in ascending parent identity (`flife-s0t`). Results are bit
/// for bit identical at any worker count.
///
/// # Errors
/// As [`resolve_births_with`], or [`CoreError::StateConflict`] if a worker
/// panicked.
pub fn resolve_births_with_workers(
    context: &BirthContext<'_>,
    first: &[(OrganismId, Prepared<'_>)],
    requests: &[BirthRequest<'_>],
    population: PopulationQuota,
    identities: &mut Identities,
    quota: &mut GenomeQuota,
    workers: usize,
) -> Result<BirthResolution, CoreError> {
    let mut none = NoWavePreparer;
    resolve_births_inner(
        context, first, &mut none, requests, population, identities, quota, workers,
    )
}

/// [`resolve_births_with_workers`] with an optional bounded-wave settle
/// preparer (FL-032, FL-043).
pub(crate) fn resolve_births_with_wave_preparer(
    context: &BirthContext<'_>,
    preparer: &mut impl FirstCandidateWave,
    requests: &[BirthRequest<'_>],
    population: PopulationQuota,
    identities: &mut Identities,
    quota: &mut GenomeQuota,
    workers: usize,
) -> Result<BirthResolution, CoreError> {
    resolve_births_inner(
        context,
        &[],
        preparer,
        requests,
        population,
        identities,
        quota,
        workers,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "private shared driver for resolve_births_with_workers and wave-bounded settle"
)]
fn resolve_births_inner(
    context: &BirthContext<'_>,
    first: &[(OrganismId, Prepared<'_>)],
    preparer: &mut impl FirstCandidateWave,
    requests: &[BirthRequest<'_>],
    population: PopulationQuota,
    identities: &mut Identities,
    quota: &mut GenomeQuota,
    workers: usize,
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
            refused: Vec::new(),
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
    // Each request ends in one birth, one rejected birth or one refused
    // birth: room for every request in each list, so none grows (FL-043).
    let mut born: Vec<Birth> = Vec::new();
    let mut rejected: Vec<RejectedBirth> = Vec::new();
    let mut refused: Vec<RefusedBirth> = Vec::new();
    reserve_births(&mut born, order.len(), "births")?;
    reserve_births(&mut rejected, order.len(), "rejected births")?;
    let refusing = context.capacity == CapacityPolicy::RefuseExcessBirths;
    if refusing {
        reserve_births(&mut refused, order.len(), "refused births")?;
        admit(context.rng, tick, population, &mut order, &mut refused)?;
    }

    let max_genome_bytes = context.mutator.maximum_genome_bytes(quota.dtype());
    let wave_size = usize::try_from(PARALLEL_PROPOSAL_BUDGET_BYTES / max_genome_bytes)
        .unwrap_or(1)
        .clamp(1, MAX_PARALLEL_WAVE);
    let parallel_workers = workers.min(wave_size).min(order.len());
    let use_wave_preparer = first.is_empty() && preparer.active();
    let use_parallel = !use_wave_preparer
        && parallel_workers > 1
        && order.len() >= MIN_PARALLEL_BIRTHS
        && matches!(context.offspring, Offspring::Inherited)
        && first.is_empty();

    if use_wave_preparer {
        for wave in order.chunks(wave_size) {
            if preparer.active() {
                preparer.prepare_wave(wave);
            }
            let wave_prepared = if preparer.active() {
                preparer.prepared()
            } else {
                Vec::new()
            };
            for request in wave {
                let prepared = wave_prepared
                    .binary_search_by_key(&request.intent.parent, |(parent, _)| *parent)
                    .ok()
                    .and_then(|at| wave_prepared.get(at))
                    .map(|(_, prepared)| *prepared);
                match stage(
                    context,
                    prepared,
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
                    Ok(Staged::Short { resource, .. }) if refusing => {
                        refused.push(RefusedBirth {
                            parent: request.intent.parent,
                            resource,
                        });
                    }
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
        }
    } else if use_parallel {
        let shortfall = stage_parallel_waves(
            context,
            &order,
            birth_tick,
            deadline,
            population,
            &mut working,
            quota,
            refusing,
            parallel_workers,
            wave_size,
            &mut born,
            &mut rejected,
            &mut refused,
        )?;
        if let Some(shortfall) = shortfall {
            return Ok(BirthResolution::Paused(shortfall));
        }
    } else {
        for request in &order {
            let prepared = first
                .binary_search_by_key(&request.intent.parent, |(parent, _)| *parent)
                .ok()
                .and_then(|at| first.get(at))
                .map(|(_, prepared)| *prepared);
            match stage(
                context,
                prepared,
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
                Ok(Staged::Short { resource, .. }) if refusing => {
                    refused.push(RefusedBirth {
                        parent: request.intent.parent,
                        resource,
                    });
                }
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
    }
    *identities = working;
    refused.sort_unstable_by_key(|birth| birth.parent);
    Ok(BirthResolution::Staged(Births {
        born,
        rejected,
        refused,
    }))
}

/// Ensures parallel proposal workers always wake from `start` and exit cleanly
/// even if serial commit exits early on a shortfall or error.
struct StopWorkers<'a> {
    stop: &'a AtomicBool,
    start: &'a Barrier,
    stopped: bool,
}

impl StopWorkers<'_> {
    fn finish(&mut self) {
        if !self.stopped {
            self.stopped = true;
            self.stop.store(true, Ordering::Release);
            self.start.wait();
        }
    }
}

impl Drop for StopWorkers<'_> {
    fn drop(&mut self) {
        self.finish();
    }
}

fn propose_safely(
    propose_one: &(impl Fn(&BirthRequest<'_>) -> Result<DetachedProposal, CoreError> + Sync),
    request: &BirthRequest<'_>,
) -> Result<DetachedProposal, CoreError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| propose_one(request))) {
        Ok(result) => result,
        Err(_) => Err(CoreError::StateConflict {
            operation: "propose births",
            detail: "a birth proposal worker panicked".into(),
        }),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "private wave coordinator for resolve_births_with_workers"
)]
fn stage_parallel_waves(
    context: &BirthContext<'_>,
    order: &[&BirthRequest<'_>],
    birth_tick: Tick,
    deadline: Tick,
    population: PopulationQuota,
    working: &mut Identities,
    quota: &mut GenomeQuota,
    refusing: bool,
    used: usize,
    wave_size: usize,
    born: &mut Vec<Birth>,
    rejected: &mut Vec<RejectedBirth>,
    refused: &mut Vec<RefusedBirth>,
) -> Result<Option<CapacityShortfall>, CoreError> {
    let dtype = quota.dtype();
    stage_parallel_waves_with(
        context,
        order,
        birth_tick,
        deadline,
        population,
        working,
        quota,
        refusing,
        used,
        wave_size,
        born,
        rejected,
        refused,
        &|request| {
            let attempt = BirthAttempt {
                parent: request.intent.parent,
                tick: request.intent.tick,
            };
            context
                .mutator
                .propose_detached(request.genome, attempt, dtype)
        },
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "private wave coordinator for resolve_births_with_workers"
)]
fn stage_parallel_waves_with(
    context: &BirthContext<'_>,
    order: &[&BirthRequest<'_>],
    birth_tick: Tick,
    deadline: Tick,
    population: PopulationQuota,
    working: &mut Identities,
    quota: &mut GenomeQuota,
    refusing: bool,
    used: usize,
    wave_size: usize,
    born: &mut Vec<Birth>,
    rejected: &mut Vec<RejectedBirth>,
    refused: &mut Vec<RefusedBirth>,
    propose_one: &(impl Fn(&BirthRequest<'_>) -> Result<DetachedProposal, CoreError> + Sync),
) -> Result<Option<CapacityShortfall>, CoreError> {
    let cursor = AtomicUsize::new(0);
    let wave_end = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let start_barrier = Barrier::new(used);
    let done_barrier = Barrier::new(used);
    let (tx, rx) = mpsc::channel::<(usize, Result<DetachedProposal, CoreError>)>();

    let mut wave_slots: Vec<Option<Result<DetachedProposal, CoreError>>> = Vec::new();
    reserve_births(&mut wave_slots, wave_size, "birth proposal wave")?;
    let mut main_results: Vec<(usize, Result<DetachedProposal, CoreError>)> = Vec::new();
    reserve_births(&mut main_results, wave_size, "birth proposal lane")?;

    let outcome = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 1..used {
            let worker_tx = tx.clone();
            let cursor = &cursor;
            let wave_end = &wave_end;
            let stop = &stop;
            let start_barrier = &start_barrier;
            let done_barrier = &done_barrier;
            handles.push(scope.spawn(move || {
                loop {
                    start_barrier.wait();
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    let end = wave_end.load(Ordering::Acquire);
                    loop {
                        let index = cursor.fetch_add(1, Ordering::Relaxed);
                        if index >= end {
                            break;
                        }
                        let Some(request) = order.get(index) else {
                            break;
                        };
                        let proposed = propose_safely(propose_one, request);
                        if worker_tx.send((index, proposed)).is_err() {
                            break;
                        }
                    }
                    done_barrier.wait();
                }
            }));
        }
        drop(tx);

        let mut guard = StopWorkers {
            stop: &stop,
            start: &start_barrier,
            stopped: false,
        };

        let mut result: Result<Option<CapacityShortfall>, CoreError> = Ok(None);
        let mut wave_start = 0;
        'waves: while wave_start < order.len() {
            let end = (wave_start + wave_size).min(order.len());
            let wave_len = end - wave_start;
            cursor.store(wave_start, Ordering::Release);
            wave_end.store(end, Ordering::Release);
            start_barrier.wait();

            main_results.clear();
            loop {
                let index = cursor.fetch_add(1, Ordering::Relaxed);
                if index >= end {
                    break;
                }
                let Some(request) = order.get(index) else {
                    break;
                };
                let proposed = propose_safely(propose_one, request);
                main_results.push((index, proposed));
            }
            done_barrier.wait();

            wave_slots.clear();
            wave_slots.resize_with(wave_len, || None);
            for (index, proposed) in main_results.drain(..).chain(rx.try_iter()) {
                if let Some(slot) = index
                    .checked_sub(wave_start)
                    .and_then(|offset| wave_slots.get_mut(offset))
                {
                    *slot = Some(proposed);
                }
            }

            for offset in 0..wave_len {
                let Some(request) = order.get(wave_start + offset) else {
                    break;
                };
                let Some(proposed) = wave_slots.get_mut(offset).and_then(Option::take) else {
                    result = Err(CoreError::StateConflict {
                        operation: "propose births",
                        detail: "a birth proposal worker did not return a result".into(),
                    });
                    break 'waves;
                };
                match stage_detached(
                    context,
                    request,
                    proposed,
                    birth_tick,
                    deadline,
                    population,
                    working,
                    quota,
                    born.len(),
                ) {
                    Ok(Staged::Born(birth)) => born.push(*birth),
                    Ok(Staged::Rejected(birth)) => rejected.push(birth),
                    Ok(Staged::Short { resource, .. }) if refusing => {
                        refused.push(RefusedBirth {
                            parent: request.intent.parent,
                            resource,
                        });
                    }
                    Ok(Staged::Short {
                        resource,
                        required,
                        available,
                    }) => {
                        let covered = born.len();
                        wave_slots.clear();
                        result = Ok(Some(CapacityShortfall {
                            resource,
                            parent: request.intent.parent,
                            required,
                            available,
                            proposed: order.iter().map(|req| req.intent.parent).collect(),
                            covered,
                        }));
                        break 'waves;
                    }
                    Err(error) => {
                        wave_slots.clear();
                        result = Err(error);
                        break 'waves;
                    }
                }
            }
            wave_start = end;
        }

        guard.finish();
        for handle in handles {
            if handle.join().is_err() && result.is_ok() {
                result = Err(CoreError::StateConflict {
                    operation: "propose births",
                    detail: "a birth proposal worker panicked".into(),
                });
            }
        }
        result
    });

    match outcome {
        Ok(None) => Ok(None),
        Ok(Some(shortfall)) => {
            let drained = std::mem::take(born);
            roll_back(drained, quota)?;
            Ok(Some(shortfall))
        }
        Err(error) => {
            let drained = std::mem::take(born);
            roll_back(drained, quota)?;
            Err(error)
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "private commit step matching stage() for a pre-proposed detached genome"
)]
fn stage_detached(
    context: &BirthContext<'_>,
    request: &BirthRequest<'_>,
    proposed: Result<DetachedProposal, CoreError>,
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
    let committed = match proposed {
        Ok(detached) => {
            context
                .mutator
                .commit_detached(detached, request.genome, attempt, genome_id, quota)
        }
        Err(error) => Err(error),
    };
    let proposal = match committed {
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

/// `refuse-excess-births-v1`'s slot admission: when `order` (ascending
/// parent identity) holds more births than `population` has free slots,
/// keep the births with the smallest admission keys, still in ascending
/// parent identity, and move the rest to `refused`.
///
/// # Errors
/// [`CoreError::AllocationFailed`] if the keys cannot be held.
fn admit(
    rng: &SemanticRng,
    tick: Tick,
    population: PopulationQuota,
    order: &mut Vec<&BirthRequest<'_>>,
    refused: &mut Vec<RefusedBirth>,
) -> Result<(), CoreError> {
    let slots = usize::try_from(population.available()).unwrap_or(usize::MAX);
    if order.len() <= slots {
        return Ok(());
    }
    let mut keys: Vec<(u64, OrganismId)> = Vec::new();
    reserve_births(&mut keys, order.len(), "admission keys")?;
    for request in order.iter() {
        let parent = request.intent.parent;
        keys.push((admission_key(rng, parent, tick)?, parent));
    }
    // The first `slots` keys are admitted; they are sorted back by parent
    // for the membership test.
    keys.sort_unstable();
    let admitted = keys.get_mut(..slots).unwrap_or_default();
    admitted.sort_unstable_by_key(|&(_, parent)| parent);
    order.retain(|request| {
        let parent = request.intent.parent;
        let keep = admitted
            .binary_search_by_key(&parent, |&(_, admitted)| admitted)
            .is_ok();
        if !keep {
            refused.push(RefusedBirth {
                parent,
                resource: Capacity::PopulationSlots,
            });
        }
        keep
    });
    Ok(())
}

/// A parent's admission key at attempt tick `tick`: word 0 of
/// `(birth, parent, n, lane (attempt, 1), 0)`.
///
/// # Errors
/// As `Lane::new`, which does not fail for index 1.
pub fn admission_key(rng: &SemanticRng, parent: OrganismId, tick: Tick) -> Result<u64, CoreError> {
    let [word, ..] = rng.block(Address {
        stream: Stream::Birth,
        entity: parent.get(),
        ordinal: tick.get(),
        lane: Lane::new(0, LaneKind::Attempt, 1)?,
        draw: 0,
    });
    Ok(word)
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
    prepared: Option<Prepared<'_>>,
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
        Offspring::Inherited => match prepared {
            Some(first) => {
                context
                    .mutator
                    .propose_prepared(request.genome, attempt, genome_id, quota, first)
            }
            None => context
                .mutator
                .propose(request.genome, attempt, genome_id, quota),
        },
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

#[cfg(test)]
mod tests {
    use super::*;
    use flife_genome::{MutationParameters, founder};

    #[test]
    fn parallel_proposal_panic_returns_state_conflict_without_deadlock_and_rolls_back() {
        let plan = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");
        let appendix = &plan[plan.find("## Appendix A").unwrap()..];
        let start = appendix.find("```toml\n").unwrap() + "```toml\n".len();
        let length = appendix[start..].find("```").unwrap();
        let validated = ValidatedConfig::from_toml(&appendix[start..start + length]).unwrap();
        let grid = FieldGrid::from_config(&validated).unwrap();
        let reproduction = Reproduction::from_config(&validated).unwrap();
        let catalog = TopologyCatalog::from_config(&validated).unwrap();
        let limits = TraitLimits::from_config(&validated).unwrap();
        let mut quota = GenomeQuota::from_config(&validated).unwrap();
        let rng = SemanticRng::new(99);
        let mutator = Mutator::new(
            rng,
            MutationParameters::from_config(&validated).unwrap(),
            &catalog,
            limits,
        )
        .unwrap();

        let mut identities = Identities {
            organisms: IdSequence::starting_at(1),
            genomes: IdSequence::starting_at(1),
        };
        let mut parents = Vec::new();
        for i in 0..8u64 {
            let id = identities.organisms.issue().unwrap();
            let gid = identities.genomes.issue().unwrap();
            let genome = founder(&rng, id, gid, &catalog, &mut quota, &limits, 1.0).unwrap();
            parents.push((id, i, genome));
        }

        let Viability::Viable(split) = reproduction.split(10.0, 1.0 / 3.0).unwrap() else {
            panic!("expected viable split");
        };
        let intents: Vec<BirthIntent> = parents
            .iter()
            .map(|(id, i, _)| BirthIntent {
                parent: *id,
                lineage: LineageId::new(1),
                generation: Generation::new(1),
                tick: Tick::new(5),
                position: [1.0 + *i as f64, 2.0],
                split,
            })
            .collect();
        let requests: Vec<BirthRequest<'_>> = intents
            .iter()
            .zip(&parents)
            .map(|(intent, (_, _, genome))| BirthRequest {
                intent: *intent,
                genome,
            })
            .collect();
        let order: Vec<&BirthRequest<'_>> = requests.iter().collect();
        let context = BirthContext {
            grid: &grid,
            reproduction: &reproduction,
            mutator: &mutator,
            rng: &rng,
            offspring: Offspring::Inherited,
            capacity: CapacityPolicy::PauseBeforeCommit,
        };

        let bytes_before = quota.allocated_bytes();
        let logical_before = quota.logical_bytes();
        let live_before = quota.live();
        let dtype = quota.dtype();

        // Panic on wave 0 (index 0), wave 0 (index 1), and wave 2 (index 5 after waves 0-1 staged births).
        for panic_parent in [parents[0].0, parents[1].0, parents[5].0] {
            let mut working = identities.clone();
            let mut born = Vec::new();
            let mut rejected = Vec::new();
            let mut refused = Vec::new();
            let result = stage_parallel_waves_with(
                &context,
                &order,
                Tick::new(6),
                Tick::new(20),
                PopulationQuota::new(32, 8).unwrap(),
                &mut working,
                &mut quota,
                false,
                4,
                2,
                &mut born,
                &mut rejected,
                &mut refused,
                &|request| {
                    if request.intent.parent == panic_parent {
                        panic!("injected proposal panic for parent {}", panic_parent.get());
                    }
                    let attempt = BirthAttempt {
                        parent: request.intent.parent,
                        tick: request.intent.tick,
                    };
                    context
                        .mutator
                        .propose_detached(request.genome, attempt, dtype)
                },
            );
            assert!(
                matches!(
                    result,
                    Err(CoreError::StateConflict {
                        operation: "propose births",
                        ..
                    })
                ),
                "expected StateConflict on panic for parent {}, got {result:?}",
                panic_parent.get()
            );
            assert!(born.is_empty(), "staged births must be drained on rollback");
            assert_eq!(
                quota.allocated_bytes(),
                bytes_before,
                "genome quota bytes must roll back after panic"
            );
            assert_eq!(
                quota.logical_bytes(),
                logical_before,
                "logical genome bytes must roll back after panic"
            );
            assert_eq!(
                quota.live(),
                live_before,
                "live genome count must roll back after panic"
            );
        }
    }
}
