//! A bounded snapshot of one organism at a committed tick (FL-021, plan
//! §14.2): what the inspector shows when an organism is selected.
//!
//! [`World::organism_snapshot`](crate::World::organism_snapshot) reads one
//! organism's row, its own genome's heritable traits and parameter count and
//! its topology's shape, and decodes its held controller output as the world
//! decodes it. It copies no controller parameter and nothing of any other
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

use flife_core::clock::Tick;
use flife_field::pyramid::RawSample;
use flife_genome::Traits;
use flife_neural::{DecodedActions, Observation};

use crate::Organism;

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
