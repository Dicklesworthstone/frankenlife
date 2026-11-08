//! Exhaustion, tiny-reservoir clearing, death and recycling (plan §4.4,
//! §7.6, §7.7, §8.1 phases 6–9, §8.2, A.1, Appendix C.12).
//!
//! §8.2 orders one organism's budget stages. They are chemistry (gain, then
//! toxicity), maintenance, motion, emission, the reproduction split and
//! finally tiny-reservoir clearing. [`Vitality`] rechecks exhaustion after
//! every stage, in that order, and remembers the first stage that exhausted
//! the budget. From then on the organism is dead. Its budget pays nothing
//! and absorbs nothing, every later stage operator finds it exhausted, and
//! nothing it already paid for is undone: motion keeps its displacement and
//! a staged deposit stands. A survivor is published only after the
//! tiny-reservoir stage, so every published organism holds strictly
//! positive energy at or above the threshold (INV-01).
//!
//! `dissipate-and-record` clears a reservoir strictly below
//! `world.tiny_energy_threshold`. The whole remainder is paid and recorded
//! as the `tiny-reservoir` flow, and the organism dies of it. An organism
//! carries no chemical moment in `field-ecology-v1`, so there is no moment
//! to clear with it.
//!
//! A [`Death`] is collected once per organism, at its resulting
//! (post-motion) position, with its cause. Natural death in this model is
//! always exhaustion, which leaves no residual and so recycles nothing.
//! An explicit removal names its [`Disposition`]:
//! - remove the residual from the modeled world (`command-removal`);
//! - dissipate it (`unrecycled-death`);
//! - or recycle it. Recycling (`genetic-affinity`) offers the configured
//!   fraction of the residual to the field at the death position, carrying
//!   the organism's own affinity direction as its chemistry. The offer goes
//!   through a `flife_field::Deposits` reduced under `Transfer::Recycling`:
//!   the field receives at most the offer, and the rest is
//!   `unrecycled-death`.
//!
//! A numerical fault is never a death. Budgets cannot hold a nonfinite
//! value, a death with a nonfinite position or residual is refused, and
//! `crate::population::Population` refuses to store a nonfinite organism,
//! naming it. Stopping the run and preserving the candidate
//! (`stop-and-preserve`) is the ordered tick's.

use flife_core::clock::Tick;
use flife_core::ids::{Generation, GenomeId, LineageId, OrganismId};
use flife_core::ledger::{EnergyBudget, TickLedger};
use flife_core::{CoreError, Flow, ValidatedConfig};
use flife_field::state::CHANNELS;
use flife_field::{Deposits, FieldGrid};

use crate::population::Organism;

/// The tiny-energy policy this module implements.
pub const DISSIPATE_AND_RECORD: &str = "dissipate-and-record";

/// The corpse direction this module implements: a recycled deposit carries
/// the organism's genetic affinity direction.
pub const GENETIC_AFFINITY: &str = "genetic-affinity";

/// One organism's ordered budget stages (§8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BudgetStage {
    /// Uptake gain, then toxicity clipped to what the organism holds.
    Chemistry,
    /// Mandatory maintenance.
    Maintenance,
    /// Affordable motion.
    Motion,
    /// Paid emission.
    Emission,
    /// The reproduction split.
    Split,
    /// Tiny-reservoir clearing, always last.
    TinyReservoir,
}

impl BudgetStage {
    /// Every stage, in order.
    pub const ALL: [Self; 6] = [
        Self::Chemistry,
        Self::Maintenance,
        Self::Motion,
        Self::Emission,
        Self::Split,
        Self::TinyReservoir,
    ];

    /// The stage's canonical name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Chemistry => "chemistry",
            Self::Maintenance => "maintenance",
            Self::Motion => "motion",
            Self::Emission => "emission",
            Self::Split => "split",
            Self::TinyReservoir => "tiny-reservoir",
        }
    }
}

/// Whether an organism survives its tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fate {
    /// It holds strictly positive energy after every stage.
    Survives,
    /// The named stage exhausted it.
    Dies(BudgetStage),
}

/// The exhaustion check after each of one organism's budget stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Vitality {
    checked: Option<BudgetStage>,
    died: Option<BudgetStage>,
}

impl Vitality {
    /// An organism alive at the start of its tick, before any stage.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            checked: None,
            died: None,
        }
    }

    /// Recheck exhaustion after `stage` has settled against `budget`.
    /// Returns whether the organism may still act. The first stage that
    /// finds the budget exhausted is the stage it died at; later stages
    /// never change that.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a stage at or before one already
    /// checked: stages run once each, in order.
    pub fn check(&mut self, stage: BudgetStage, budget: &EnergyBudget) -> Result<bool, CoreError> {
        if let Some(previous) = self.checked
            && stage <= previous
        {
            return Err(CoreError::StateConflict {
                operation: "check exhaustion",
                detail: format!(
                    "stage {} cannot follow stage {}",
                    stage.name(),
                    previous.name()
                ),
            });
        }
        self.checked = Some(stage);
        if self.died.is_none() && budget.is_exhausted() {
            self.died = Some(stage);
        }
        Ok(self.died.is_none())
    }

    /// The stage that exhausted the organism, if one has.
    #[must_use]
    pub const fn died_at(&self) -> Option<BudgetStage> {
        self.died
    }

    /// Whether no stage has exhausted the organism yet.
    #[must_use]
    pub const fn is_alive(&self) -> bool {
        self.died.is_none()
    }

    /// The organism's fate once every stage has been checked.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a survivor whose tiny-reservoir
    /// stage has not been checked: nothing is published before it.
    pub fn fate(&self) -> Result<Fate, CoreError> {
        if let Some(stage) = self.died {
            return Ok(Fate::Dies(stage));
        }
        if self.checked == Some(BudgetStage::TinyReservoir) {
            Ok(Fate::Survives)
        } else {
            Err(CoreError::StateConflict {
                operation: "decide fate",
                detail: "the tiny-reservoir stage has not been checked".into(),
            })
        }
    }
}

/// `dissipate-and-record` (§8.2, A.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TinyEnergyPolicy {
    threshold: f64,
}

impl TinyEnergyPolicy {
    /// Clear reservoirs strictly below `threshold`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `threshold` is finite and
    /// nonnegative.
    pub fn new(threshold: f64) -> Result<Self, CoreError> {
        if !(threshold.is_finite() && threshold >= 0.0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "world.tiny_energy_threshold",
                detail: format!("{threshold} is not a finite nonnegative energy"),
            });
        }
        Ok(Self {
            threshold: threshold + 0.0,
        })
    }

    /// The policy of a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another policy; otherwise as
    /// [`TinyEnergyPolicy::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let world = &config.config().world;
        if world.tiny_energy_policy != DISSIPATE_AND_RECORD {
            return Err(CoreError::UnknownAlgorithm {
                role: "world.tiny_energy_policy",
                name: world.tiny_energy_policy.clone(),
            });
        }
        Self::new(world.tiny_energy_threshold)
    }

    /// The threshold.
    #[must_use]
    pub const fn threshold(&self) -> f64 {
        self.threshold
    }

    /// The last budget stage: clear `budget` if it holds strictly less than
    /// the threshold. Returns the cleared energy, which the caller records
    /// as [`Flow::TinyReservoir`]. A cleared budget is exhausted.
    ///
    /// # Errors
    /// None in practice; payments of a budget's own balance always succeed.
    pub fn clear(&self, budget: &mut EnergyBudget) -> Result<f64, CoreError> {
        let remaining = budget.remaining();
        if budget.is_exhausted() || remaining >= self.threshold {
            return Ok(0.0);
        }
        let paid = budget.pay(remaining)?;
        Ok(paid.paid)
    }
}

/// What an explicit removal does with the organism's residual energy
/// (§7.6). The choice is recorded with the death.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Disposition {
    /// Remove it from the modeled world (`command-removal`, `O`).
    Remove,
    /// Dissipate it (`unrecycled-death`, `D`).
    Dissipate,
    /// Recycle the configured fraction into the field; the rest dissipates.
    Recycle,
}

/// Why an organism died.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeathCause {
    /// The named budget stage exhausted it.
    Exhausted(BudgetStage),
    /// It was explicitly removed.
    Removed(Disposition),
}

/// One collected death (§7.6, §7.7). Organism, genome and lineage identity
/// are kept apart: the genome can outlive the organism in checkpoints and
/// descendants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Death {
    /// The organism.
    pub id: OrganismId,
    /// Its genome.
    pub genome: GenomeId,
    /// Its founder lineage.
    pub lineage: LineageId,
    /// Its generation.
    pub generation: Generation,
    /// The committed tick it is absent from: `n + 1` for a death during the
    /// advance from `n`.
    pub tick: Tick,
    /// Its resulting position, where it is collected.
    pub position: [f64; 2],
    /// Why it died.
    pub cause: DeathCause,
    /// The energy it still held: zero after exhaustion.
    pub residual: f64,
}

impl Death {
    /// `organism` exhausted at `stage` during the advance to `tick`, at its
    /// resulting `position`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite position.
    pub fn exhausted(
        organism: &Organism,
        tick: Tick,
        position: [f64; 2],
        stage: BudgetStage,
    ) -> Result<Self, CoreError> {
        Self::collect(organism, tick, position, DeathCause::Exhausted(stage), 0.0)
    }

    /// `organism` explicitly removed at `tick` with `residual` energy and the
    /// chosen `disposition`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite position or a
    /// negative or nonfinite residual.
    pub fn removed(
        organism: &Organism,
        tick: Tick,
        position: [f64; 2],
        residual: f64,
        disposition: Disposition,
    ) -> Result<Self, CoreError> {
        Self::collect(
            organism,
            tick,
            position,
            DeathCause::Removed(disposition),
            residual,
        )
    }

    fn collect(
        organism: &Organism,
        tick: Tick,
        position: [f64; 2],
        cause: DeathCause,
        residual: f64,
    ) -> Result<Self, CoreError> {
        if !(position[0].is_finite() && position[1].is_finite()) {
            return Err(CoreError::NumericalInvalid {
                quantity: "death position",
                detail: format!("{}: {position:?} is not finite", organism.id),
            });
        }
        if !(residual.is_finite() && residual >= 0.0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "residual energy",
                detail: format!(
                    "{}: {residual} is not a finite nonnegative energy",
                    organism.id
                ),
            });
        }
        Ok(Self {
            id: organism.id,
            genome: organism.genome,
            lineage: organism.lineage,
            generation: organism.generation,
            tick,
            position,
            cause,
            residual: residual + 0.0,
        })
    }
}

/// The `[death]` recycling rule with the `genetic-affinity` corpse direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Recycling {
    fraction: f64,
}

impl Recycling {
    /// Recycle `fraction ∈ [0, 1]` of a recycled residual.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] outside `[0, 1]`.
    pub fn new(fraction: f64) -> Result<Self, CoreError> {
        if !(0.0..=1.0).contains(&fraction) {
            return Err(CoreError::NumericalInvalid {
                quantity: "death.residual_energy_recycle_fraction",
                detail: format!("{fraction} is outside [0, 1]"),
            });
        }
        Ok(Self {
            fraction: fraction + 0.0,
        })
    }

    /// The rule of a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another corpse direction;
    /// otherwise as [`Recycling::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let death = &config.config().death;
        if death.corpse_direction != GENETIC_AFFINITY {
            return Err(CoreError::UnknownAlgorithm {
                role: "death.corpse_direction",
                name: death.corpse_direction.clone(),
            });
        }
        Self::new(death.residual_energy_recycle_fraction)
    }

    /// The recycled fraction.
    #[must_use]
    pub const fn fraction(&self) -> f64 {
        self.fraction
    }

    /// Settle `death`'s residual. A recycled residual is staged in
    /// `deposits` at the death position with `affinity`, the organism's
    /// genetic affinity direction, as its chemistry.
    ///
    /// # Errors
    /// - [`CoreError::StateConflict`] for an exhaustion death that claims a
    ///   residual.
    /// - [`CoreError::NumericalInvalid`] for an affinity outside the unit
    ///   ball, as `flife_field::Deposits::push`. Nothing is staged on error.
    pub fn settle(
        &self,
        grid: &FieldGrid,
        death: &Death,
        affinity: &[f64; CHANNELS],
        deposits: &mut Deposits,
    ) -> Result<DeathSettlement, CoreError> {
        let residual = death.residual;
        let mut settlement = DeathSettlement {
            residual,
            removed: 0.0,
            dissipated: 0.0,
            recycled: 0.0,
            offered: 0.0,
        };
        match death.cause {
            DeathCause::Exhausted(_) if residual > 0.0 => Err(CoreError::StateConflict {
                operation: "settle death",
                detail: format!("{} died exhausted but holds {residual}", death.id),
            }),
            DeathCause::Exhausted(_) => Ok(settlement),
            DeathCause::Removed(Disposition::Remove) => {
                settlement.removed = residual;
                Ok(settlement)
            }
            DeathCause::Removed(Disposition::Dissipate) => {
                settlement.dissipated = residual;
                Ok(settlement)
            }
            DeathCause::Removed(Disposition::Recycle) => {
                if residual > 0.0 {
                    let offered = self.fraction * residual;
                    deposits.push(grid, death.id, death.position, residual, offered, affinity)?;
                    settlement.recycled = residual;
                    settlement.offered = offered;
                }
                Ok(settlement)
            }
        }
    }
}

/// Where one death's residual went.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeathSettlement {
    /// The residual energy.
    pub residual: f64,
    /// Removed from the modeled world.
    pub removed: f64,
    /// Dissipated outright.
    pub dissipated: f64,
    /// Staged for recycling. The recycling reduction records it.
    pub recycled: f64,
    /// The recycled fraction of it, offered to the field.
    pub offered: f64,
}

impl DeathSettlement {
    /// Record the removed and dissipated parts. The recycled part is the
    /// recycling reduction's `Transfer::Recycling`, recorded through
    /// `flife_field::DepositSummary::record`.
    ///
    /// # Errors
    /// As [`TickLedger::record`].
    pub fn record(&self, ledger: &mut TickLedger) -> Result<(), CoreError> {
        if self.removed > 0.0 {
            ledger.record(Flow::CommandRemoval, self.removed)?;
        }
        if self.dissipated > 0.0 {
            ledger.record(Flow::UnrecycledDeath, self.dissipated)?;
        }
        Ok(())
    }
}
