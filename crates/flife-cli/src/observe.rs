//! What a run did, from its published reports and its committed raw state
//! (plan §15.1, §15.2; FL-015).
//!
//! A [`Tally`] folds the published tick reports: births, rejected births,
//! deaths by budget stage, emitters, paid and deposited emission energy, the
//! mutation counts the births recorded, and the live population's range.
//!
//! A [`Survey`] reads one committed world directly, without the reports:
//! the live organisms' generations and lineages, how many identities were
//! issued, how many distinct genomes are alive, and [`Heredity`]: live
//! parent–child pairs and grandparent–parent–child triples compared over
//! their genomes, against unrelated founders as the baseline. A child that
//! differs from its parent in a few parameters, equals it in the rest, and
//! sits much closer to it in trait space than unrelated founders do shows
//! variation and inheritance in raw state. A child that also carries the
//! parameters where its parent differs from its grandparent, and is closer
//! to its parent than to its grandparent in trait space, shows that the
//! variation is heritable.

use std::collections::{BTreeMap, BTreeSet};

use flife_core::canonical::Sha256Digest;
use flife_core::ids::OrganismId;
use flife_core::ledger::Transfer;
use flife_core::numerics;
use flife_engine::{DeathCause, Population, TickReport, World};
use flife_genome::Genome;
use serde::Serialize;

/// The published reports of a run, folded.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Tally {
    /// Published ticks.
    pub ticks: u64,
    /// Births committed.
    pub births: u64,
    /// Viable attempts whose every mutation candidate was rejected.
    pub rejected_births: u64,
    /// Deaths committed.
    pub deaths: u64,
    /// Deaths by the budget stage that exhausted them, or `removed`.
    pub deaths_by_stage: BTreeMap<&'static str, u64>,
    /// Organism-ticks with a paid emission.
    pub emitter_ticks: u64,
    /// Energy organisms paid to emit.
    pub emission_paid: f64,
    /// Of that, the energy deposited into the field.
    pub emission_deposited: f64,
    /// Energy parents passed to children at birth.
    pub child_endowment: f64,
    /// Energy dead organisms returned to the field.
    pub recycled: f64,
    /// Parameters perturbed by the births' accepted candidates.
    pub perturbed_parameters: u64,
    /// Parameters replaced by them.
    pub replaced_parameters: u64,
    /// Births whose child changed topology.
    pub topology_changes: u64,
    /// Mutation candidates rejected before the accepted one.
    pub rejected_candidates: u64,
    /// The largest live population after a tick.
    pub peak_live: usize,
    /// The smallest live population after a tick.
    pub least_live: Option<usize>,
    /// The run ledger's gated residual after the last tick.
    pub normalized_residual: Option<f64>,
}

impl Tally {
    /// Fold one published tick's report.
    pub fn record(&mut self, report: &TickReport) {
        self.ticks += 1;
        self.births += count(report.births.len());
        self.rejected_births += count(report.rejected.len());
        self.deaths += count(report.deaths.len());
        for death in &report.deaths {
            let stage = match death.cause {
                DeathCause::Exhausted(stage) => stage.name(),
                DeathCause::Removed(_) => "removed",
            };
            *self.deaths_by_stage.entry(stage).or_insert(0) += 1;
        }
        self.emitter_ticks += count(report.emitted);
        self.emission_paid += report.closure.debited(Transfer::Emission);
        self.emission_deposited += report.closure.credited(Transfer::Emission);
        self.child_endowment += report.closure.credited(Transfer::ChildEndowment);
        self.recycled += report.closure.credited(Transfer::Recycling);
        for birth in &report.births {
            self.perturbed_parameters += u64::from(birth.record.perturbed);
            self.replaced_parameters += u64::from(birth.record.replaced);
            self.topology_changes += u64::from(birth.record.topology.is_some());
            self.rejected_candidates += count(birth.record.rejected.len());
        }
        self.peak_live = self.peak_live.max(report.live);
        self.least_live = Some(
            self.least_live
                .map_or(report.live, |least| least.min(report.live)),
        );
        self.normalized_residual = report.ledger.normalized;
    }
}

fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// Genome comparisons between live relatives, with unrelated founders as
/// the baseline.
///
/// Parameters are compared bit for bit: a child copies its parent's
/// parameters and a birth perturbs or replaces a few of them. Traits are
/// compared by distance, because every birth perturbs the whole affinity
/// and motion-response vectors and the sensing scale a little. The trait
/// vector is the affinity, the motion response and `ln(sensing scale)`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Heredity {
    /// Live parent–child pairs with the same topology.
    pub pairs: u64,
    /// Of those, pairs whose genomes differ anywhere.
    pub pairs_differing: u64,
    /// Live parent–child pairs whose topologies differ.
    pub pairs_across_topologies: u64,
    /// Parameters compared over every pair.
    pub pair_parameters: u64,
    /// Of those, parameters where child and parent are bit-identical.
    pub pair_parameters_identical: u64,
    /// The most parameters differing in one pair.
    pub most_parameter_differences: u64,
    /// The mean trait-vector distance from child to parent.
    pub pair_trait_distance: Option<f64>,
    /// Consecutive live founders of one topology, in identity order.
    pub founder_pairs: u64,
    /// Parameters compared over the founder pairs.
    pub founder_parameters: u64,
    /// Of those, bit-identical parameters.
    pub founder_parameters_identical: u64,
    /// The mean trait-vector distance between the founder pairs.
    pub founder_trait_distance: Option<f64>,
    /// Live grandparent–parent–child triples with one topology.
    pub triples: u64,
    /// Parameters where a triple's parent differs from its grandparent.
    pub parental_parameter_variants: u64,
    /// Of those, the parameters the child carries: equal to its parent's.
    pub carried_parameter_variants: u64,
    /// The mean trait-vector distance from each triple's child to its
    /// parent.
    pub triple_trait_distance_to_parent: Option<f64>,
    /// The mean trait-vector distance from each triple's child to its
    /// grandparent.
    pub triple_trait_distance_to_grandparent: Option<f64>,
}

/// One committed world's raw state.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Survey {
    /// The committed tick.
    pub tick: u64,
    /// Live organisms.
    pub live: usize,
    /// Live founders (generation 0).
    pub founders_alive: usize,
    /// Live descendants (generation 1 and later).
    pub descendants_alive: usize,
    /// The largest live generation.
    pub max_generation: u32,
    /// Live organisms by generation.
    pub generations: BTreeMap<u32, usize>,
    /// Founder lineages with a live member.
    pub lineages_alive: usize,
    /// Organism identities issued since the world was founded: founders
    /// plus births.
    pub identities_issued: u64,
    /// Distinct genome contents among the live organisms: topology,
    /// parameters and traits together.
    pub distinct_genomes: usize,
    /// Distinct `(topology, parameters)` among them.
    pub distinct_parameter_sets: usize,
    /// Distinct trait vectors among them.
    pub distinct_trait_vectors: usize,
    /// Live organisms by topology, in catalog order.
    pub topologies: BTreeMap<usize, usize>,
    /// The genome comparisons.
    pub heredity: Heredity,
}

impl Survey {
    /// Survey `world`'s committed state.
    #[must_use]
    pub fn of(world: &World) -> Self {
        let population = world.population();
        let mut generations = BTreeMap::new();
        let mut lineages = BTreeSet::new();
        let mut contents = BTreeSet::new();
        let mut parameter_sets = BTreeSet::new();
        let mut trait_vectors = BTreeSet::new();
        let mut topologies = BTreeMap::new();
        let mut founders_alive = 0;
        for organism in population.iter() {
            *generations.entry(organism.generation.get()).or_insert(0) += 1;
            lineages.insert(organism.lineage.get());
            if organism.parent.is_none() {
                founders_alive += 1;
            }
            if let Some(genome) = population.genome(organism.id) {
                let parameters = (genome.topology().index(), parameter_digest(genome));
                let traits = trait_digest(genome);
                contents.insert((parameters, traits));
                parameter_sets.insert(parameters);
                trait_vectors.insert(traits);
                *topologies.entry(genome.topology().index()).or_insert(0) += 1;
            }
        }
        let live = population.len();
        Self {
            tick: world.tick().get(),
            live,
            founders_alive,
            descendants_alive: live - founders_alive,
            max_generation: generations.keys().next_back().copied().unwrap_or(0),
            generations,
            lineages_alive: lineages.len(),
            identities_issued: world.identities().organisms.next_raw().unwrap_or(u64::MAX),
            distinct_genomes: contents.len(),
            distinct_parameter_sets: parameter_sets.len(),
            distinct_trait_vectors: trait_vectors.len(),
            topologies,
            heredity: heredity(population),
        }
    }
}

#[derive(Default)]
struct Mean {
    sum: f64,
    count: u64,
}

impl Mean {
    fn add(&mut self, value: f64) {
        self.sum += value;
        self.count += 1;
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "a count of organism pairs is far below 2^53"
    )]
    fn get(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / self.count as f64)
    }
}

fn heredity(population: &Population) -> Heredity {
    let mut heredity = Heredity::default();
    let mut pair_traits = Mean::default();
    let mut founder_traits = Mean::default();
    let mut to_parent = Mean::default();
    let mut to_grandparent = Mean::default();
    let mut previous_founder: Option<&Genome> = None;
    for organism in population.iter() {
        let Some(genome) = population.genome(organism.id) else {
            continue;
        };
        let Some(parent_id) = organism.parent else {
            if let Some(previous) = previous_founder
                && previous.topology() == genome.topology()
            {
                let (compared, identical) = compare(previous, genome);
                heredity.founder_pairs += 1;
                heredity.founder_parameters += compared;
                heredity.founder_parameters_identical += identical;
                founder_traits.add(trait_distance(previous, genome));
            }
            previous_founder = Some(genome);
            continue;
        };
        let Some(parent) = population.genome(parent_id) else {
            continue;
        };
        if parent.topology() != genome.topology() {
            heredity.pairs_across_topologies += 1;
            continue;
        }
        let (compared, identical) = compare(parent, genome);
        let distance = trait_distance(parent, genome);
        heredity.pairs += 1;
        heredity.pair_parameters += compared;
        heredity.pair_parameters_identical += identical;
        heredity.pairs_differing += u64::from(identical < compared || distance > 0.0);
        heredity.most_parameter_differences = heredity
            .most_parameter_differences
            .max(compared - identical);
        pair_traits.add(distance);
        if let Some(grandparent) = grandparent(population, parent_id)
            && grandparent.topology() == genome.topology()
        {
            let (variants, carried) = carried(grandparent, parent, genome);
            heredity.triples += 1;
            heredity.parental_parameter_variants += variants;
            heredity.carried_parameter_variants += carried;
            to_parent.add(distance);
            to_grandparent.add(trait_distance(grandparent, genome));
        }
    }
    heredity.pair_trait_distance = pair_traits.get();
    heredity.founder_trait_distance = founder_traits.get();
    heredity.triple_trait_distance_to_parent = to_parent.get();
    heredity.triple_trait_distance_to_grandparent = to_grandparent.get();
    heredity
}

fn grandparent(population: &Population, parent: OrganismId) -> Option<&Genome> {
    let grandparent = population.get(parent)?.parent?;
    population.genome(grandparent)
}

/// `(parameters, bit-identical parameters)` between two genomes of one
/// topology.
fn compare(a: &Genome, b: &Genome) -> (u64, u64) {
    a.parameters()
        .iter()
        .zip(b.parameters())
        .fold((0, 0), |(total, same), (x, y)| {
            (total + 1, same + u64::from(x.to_bits() == y.to_bits()))
        })
}

/// `(parameters where parent ≠ grandparent, of those where child = parent)`.
fn carried(grandparent: &Genome, parent: &Genome, child: &Genome) -> (u64, u64) {
    grandparent
        .parameters()
        .iter()
        .zip(parent.parameters())
        .zip(child.parameters())
        .filter(|((g, p), _)| g.to_bits() != p.to_bits())
        .fold((0, 0), |(variants, carried), ((_, p), c)| {
            (
                variants + 1,
                carried + u64::from(p.to_bits() == c.to_bits()),
            )
        })
}

/// The trait vector: affinity, motion response, then `ln(sensing scale)`
/// by the host-independent [`numerics::ln`], so every survey is identical on
/// every host.
fn trait_vector(genome: &Genome) -> impl Iterator<Item = f64> + '_ {
    let traits = genome.traits();
    traits
        .affinity()
        .iter()
        .copied()
        .chain(traits.motion_response().iter().copied())
        .chain(std::iter::once(
            numerics::ln(traits.sensing_scale()).unwrap_or(f64::NAN),
        ))
}

/// The Euclidean distance between two genomes' trait vectors, summed in
/// vector order.
fn trait_distance(a: &Genome, b: &Genome) -> f64 {
    trait_vector(a)
        .zip(trait_vector(b))
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f64>()
        .sqrt()
}

/// The SHA-256 of a genome's parameters' bits.
fn parameter_digest(genome: &Genome) -> Sha256Digest {
    let mut bytes = Vec::with_capacity(4 * genome.parameters().len());
    for value in genome.parameters() {
        bytes.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    Sha256Digest::of(&bytes)
}

/// The SHA-256 of a genome's trait vector's bits.
fn trait_digest(genome: &Genome) -> Sha256Digest {
    let mut bytes = Vec::with_capacity(8 * 80);
    for value in trait_vector(genome) {
        bytes.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    Sha256Digest::of(&bytes)
}
