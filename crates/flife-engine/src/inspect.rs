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

use flife_core::clock::Tick;
use flife_genome::Traits;
use flife_neural::DecodedActions;

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
}
