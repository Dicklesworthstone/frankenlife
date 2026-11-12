//! A bounded snapshot of one organism at a committed tick (FL-021, plan
//! §14.2): what the inspector shows when an organism is selected.
//!
//! [`World::organism_snapshot`](crate::World::organism_snapshot) reads one
//! organism's row, its own genome's heritable traits and parameter count and
//! its topology's shape, and decodes its held controller output as the world
//! decodes it. It reads the committed field only at the organism's exchange
//! footprint, at most four cells, for what chemistry would make of them
//! ([`LocalChemistry`]). It copies no controller parameter and nothing of any other
//! organism or genome, so its cost does not grow with the population or the
//! genome bank, and it changes nothing: the world's digest is the same after
//! it (plan §21.2, INV-09).
//!
//! What an organism sensed and how its controller responded exist only
//! inside a step: the field it observed is the one after that step's
//! transport and sources, which the step's uptake and deposits then change.
//! So the world keeps them for one organism it is told to watch
//! ([`World::watch`](crate::World::watch)): each step records that
//! organism's raw sample and encoded observation, exactly as its decision
//! read them, and publishing the step makes the record visible. A snapshot
//! carries the newest one as [`Sensed`], and, when its held output came from
//! an observation the world recorded, the hidden [`Activations`] that
//! evaluation produced: the reference controller evaluated again on those
//! inputs, which reproduces the held output bit for bit or the snapshot is
//! refused. Watching costs one organism's sample a step and changes no
//! result of a run.
//!
//! The watch also keeps what the organism's budget stages paid and received
//! in each of its last [`RECENT_STEPS`] published steps, as
//! [`Transactions`]: the settlements its chemistry, maintenance, motion and
//! emission stages returned, what its reproduction came to, and what the
//! tiny reservoir took. They are the values the ledger was given, copied as
//! they were recorded.

use flife_core::clock::Tick;
use flife_core::ids::OrganismId;
use flife_field::pyramid::RawSample;
use flife_genome::Traits;
use flife_neural::{DecodedActions, Observation};

use crate::{
    EmissionSettlement, MaintenanceSettlement, MotionSettlement, Organism, Split, Unviable,
    UptakeSettlement,
};

/// How many published steps of a watched organism's [`Transactions`] the
/// world keeps: one second at the reference profile's 1/128 s tick.
pub const RECENT_STEPS: usize = 128;

/// One organism at a committed tick.
#[derive(Debug, Clone, PartialEq)]
pub struct OrganismSnapshot {
    /// The committed tick it was read at: the world's.
    pub tick: Tick,
    /// Its row: identity, genome, parent, lineage, generation and birth
    /// tick; position, velocity, energy and its energy's change rate over
    /// its last committed tick; its held output, phase and reproduction
    /// cooldown.
    pub organism: Organism,
    /// Ticks from its birth tick to `tick`.
    pub age_ticks: u64,
    /// Its controller topology's index in the catalog.
    pub topology: usize,
    /// That topology's hidden layer widths, in evaluation order.
    pub hidden_widths: Vec<u32>,
    /// Its controller's parameter count; the parameters are not copied.
    pub parameters: usize,
    /// Its heritable affinity direction, motion response and sensing scale.
    pub traits: Traits,
    /// What the committed field offers it where it stands, for its affinity.
    pub local: LocalChemistry,
    /// Its held controller output, decoded as the world decodes it for the
    /// tick after `tick` unless it is evaluated again; `None` while it holds
    /// no output.
    pub decoded: Option<DecodedActions>,
    /// The newest observation the world recorded for it while watching it,
    /// at or before `tick`; `None` before its first watched step.
    pub sensed: Option<Sensed>,
    /// The hidden activations of the evaluation its held output came from,
    /// when the world recorded that evaluation's observation; `None`
    /// otherwise.
    pub activations: Option<Activations>,
    /// What its budget stages paid and received in each step the world
    /// recorded while watching it, oldest first: at most [`RECENT_STEPS`],
    /// the newest the step to `tick`. Empty before its first watched step.
    pub transactions: Vec<Transactions>,
}

/// What the committed field offers one organism where it stands (plan
/// §14.2's Chemistry): its `grid-cic-v1` exchange footprint on the field at
/// the snapshot's tick, each footprint cell's efficiency for its affinity,
/// and A.1's request at the footprint's density.
///
/// The field is the committed one, which the Studio draws. A step's uptake
/// reads the field after that step's transport and sources, and grants less
/// than the request where organisms contend or quanta round, so this is what
/// the organism's surroundings hold now, not a forecast of its next
/// settlement; [`Transactions`] records those.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalChemistry {
    /// The footprint's cells in ascending index order, each with a positive
    /// weight; the weights sum to one up to F64 rounding.
    pub cells: Vec<ExchangeCell>,
    /// The footprint's energy density `r = Σ_c w_c e_c / (h_x h_y)`.
    pub density: f64,
    /// A.1's request at `r`, `d = Δt U_max r/(K + r)`: what it would ask the
    /// field for in one tick from here.
    pub request: f64,
    /// `Σ_c w_c max(η_c, 0)`: the useful gain per unit of energy taken from
    /// the footprint in proportion to its weights.
    pub gain: f64,
    /// `κ Σ_c w_c max(−η_c, 0)`: the toxicity requested per unit taken in
    /// that proportion.
    pub toxicity: f64,
}

/// One cell of an organism's exchange footprint, as [`LocalChemistry`]
/// reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExchangeCell {
    /// The cell's index in the field.
    pub cell: usize,
    /// Its weight `w_c` in the footprint.
    pub weight: f64,
    /// Its energy `e_c`.
    pub energy: f32,
    /// `η(z_c) = η_max tanh(k a·z_c − b)` at its composition
    /// `z_c = m_c / e_c`, which is neutral for an empty cell (C.3).
    pub efficiency: f64,
}

/// What one watched organism's budget stages paid and received in one step
/// (plan §14.2's Transactions), in stage order (§8.2). A stage that an
/// earlier stage's exhaustion stopped is `None`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transactions {
    /// The committed tick the step started from.
    pub tick: Tick,
    /// Its energy when the step started.
    pub opening: f64,
    /// Chemistry: the uptake it absorbed, the gain credited and the
    /// toxicity paid.
    pub uptake: UptakeSettlement,
    /// Maintenance: basal, sensor and controller costs, requested and paid.
    pub maintenance: Option<MaintenanceSettlement>,
    /// Motion: the proposed and applied velocity and its cost.
    pub motion: Option<MotionSettlement>,
    /// Emission: the budget it spent from, the cost and what it offered the
    /// field.
    pub emission: Option<EmissionSettlement>,
    /// What its reproduction stage came to.
    pub reproduction: Option<ReproductionOutcome>,
    /// What the tiny reservoir took (§8.2's last stage); zero if nothing.
    pub tiny: f64,
    /// Its energy when the step ended; zero if the step exhausted it.
    pub closing: f64,
}

/// What a watched organism's reproduction stage came to in one step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReproductionOutcome {
    /// The step's tick was before its cooldown deadline.
    CoolingDown {
        /// The deadline.
        deadline: Tick,
    },
    /// Eligible, but the step's draw made no attempt.
    NoAttempt,
    /// It attempted, but no viable birth fitted its budget.
    Unviable(Unviable),
    /// A child was born: the split the parent paid.
    Born {
        /// The child.
        child: OrganismId,
        /// The energy split.
        split: Split,
    },
    /// Every candidate genome was rejected: no child and no charge.
    Rejected {
        /// The split the attempt proposed.
        split: Split,
        /// How many candidates were rejected.
        candidates: usize,
    },
}

/// One watched organism's observation in one step (plan §14.2's Inputs).
#[derive(Debug, Clone, PartialEq)]
pub struct Sensed {
    /// The committed tick the step started from: it observed the field of
    /// the step to the next tick, after that step's transport and sources.
    pub tick: Tick,
    /// Its sensing scale: where on the pyramid its sample blends.
    pub scale: f64,
    /// The blended densities `R, M_0, …, M_31` and their derivatives.
    pub raw: RawSample,
    /// The encoded observation: the 104 inputs in ABI order and the
    /// unsaturated gradients motion follows.
    pub observation: Observation,
    /// Whether it evaluated its controller on it, or held an earlier output.
    pub evaluated: bool,
}

/// The hidden activations of one evaluation (plan §14.2's Brain).
#[derive(Debug, Clone, PartialEq)]
pub struct Activations {
    /// The tick the evaluation's output is held from: one after the
    /// observation's.
    pub tick: Tick,
    /// Each hidden layer's activations, in evaluation order.
    pub hidden: Vec<Vec<f32>>,
}
