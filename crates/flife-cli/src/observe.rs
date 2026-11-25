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
use flife_core::{numerics, pool};
use flife_engine::{DeathCause, TickReport, World};
use flife_genome::{AFFINITY_TERMS, Genome, MOTION_RESPONSE_TERMS, TraitLimits, Weights};
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
    /// Viable attempts `refuse-excess-births-v1` refused for capacity.
    pub refused_births: u64,
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
        self.refused_births += count(report.refused.len());
        self.deaths += count(report.deaths.len());
        for death in &report.deaths {
            let stage = match death.cause {
                DeathCause::Exhausted(stage) => stage.name(),
                DeathCause::Lysed(_) => "lysed",
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
/// and motion-response vectors, the sensing scale, and any active
/// `field-ecology-v2` heritable traits (`pcp-polarity` and `cadherin-4d`) a
/// little. The trait vector is the affinity, the motion response,
/// `ln(sensing scale)`, and (when present) polarity strength, wedge curvature,
/// 4-D cadherin tag and cohesiveness.
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

struct SurveyRow<'a> {
    id: OrganismId,
    parent: Option<OrganismId>,
    genome: Option<&'a Genome>,
}

enum HeredityTask<'a> {
    FounderPair {
        previous: &'a Genome,
        current: &'a Genome,
    },
    ParentPair {
        parent: &'a Genome,
        child: &'a Genome,
        grandparent: Option<&'a Genome>,
    },
}

#[derive(Clone, Copy)]
enum HeredityOutcome {
    FounderPair {
        compared: u64,
        identical: u64,
        distance: f64,
    },
    ParentPair {
        compared: u64,
        identical: u64,
        distance: f64,
        traits_differ: bool,
        triple: Option<(u64, u64, f64)>,
    },
}

impl Survey {
    /// Survey `world`'s committed state.
    #[must_use]
    pub fn of(world: &World) -> Self {
        Self::of_with_workers(world, world.cpu_workers().max(1))
    }

    /// Survey `world`'s committed state using up to `workers` worker threads.
    /// Produces bit-identical results for every worker count because all
    /// floating-point reductions fold in canonical ascending identity order.
    #[must_use]
    fn of_with_workers(world: &World, workers: usize) -> Self {
        let population = world.population();
        let live = population.len();
        let mut generations = BTreeMap::new();
        let mut lineages = BTreeSet::new();
        let mut topologies = BTreeMap::new();
        let mut founders_alive = 0;
        let mut rows = Vec::with_capacity(live);
        let mut genomes: Vec<&Genome> = Vec::with_capacity(live);
        for (organism, genome) in population.iter_with_genomes() {
            *generations.entry(organism.generation.get()).or_insert(0) += 1;
            lineages.insert(organism.lineage.get());
            if organism.parent.is_none() {
                founders_alive += 1;
            }
            if let Some(genome) = genome {
                *topologies.entry(genome.topology().index()).or_insert(0) += 1;
                genomes.push(genome);
            }
            rows.push(SurveyRow {
                id: organism.id,
                parent: organism.parent,
                genome,
            });
        }

        let mut contents = compute_digests(&genomes, workers);
        contents.sort_unstable();
        contents.dedup();
        let distinct_genomes = contents.len();
        let mut distinct_parameter_sets = 0;
        let mut last_params: Option<(usize, Sha256Digest)> = None;
        for &(params, _) in &contents {
            if last_params != Some(params) {
                distinct_parameter_sets += 1;
                last_params = Some(params);
            }
        }
        let mut trait_vectors: Vec<Sha256Digest> =
            contents.iter().map(|&(_, traits)| traits).collect();
        trait_vectors.sort_unstable();
        trait_vectors.dedup();
        let distinct_trait_vectors = trait_vectors.len();

        Self {
            tick: world.tick().get(),
            live,
            founders_alive,
            descendants_alive: live - founders_alive,
            max_generation: generations.keys().next_back().copied().unwrap_or(0),
            generations,
            lineages_alive: lineages.len(),
            identities_issued: world.identities().organisms.next_raw().unwrap_or(u64::MAX),
            distinct_genomes,
            distinct_parameter_sets,
            distinct_trait_vectors,
            topologies,
            heredity: heredity(&rows, world.trait_limits(), workers),
        }
    }
}

type DigestKey = ((usize, Sha256Digest), Sha256Digest);

fn compute_digests(genomes: &[&Genome], workers: usize) -> Vec<DigestKey> {
    if genomes.is_empty() {
        return Vec::new();
    }
    let zero_digest = Sha256Digest::from_bytes([0_u8; 32]);
    let mut out = vec![((0_usize, zero_digest), zero_digest); genomes.len()];
    let compute_chunk = |slice: &[&Genome], dst: &mut [DigestKey]| {
        let mut scratch = Vec::new();
        for (&genome, slot) in slice.iter().zip(dst.iter_mut()) {
            let params = (
                genome.topology().index(),
                parameter_digest_into(genome, &mut scratch),
            );
            let traits = trait_digest_into(genome, &mut scratch);
            *slot = (params, traits);
        }
    };
    if workers <= 1 || genomes.len() < 128 {
        compute_chunk(genomes, &mut out);
        return out;
    }
    let tile_count = workers
        .saturating_mul(4)
        .min(64)
        .min(genomes.len().div_ceil(64))
        .max(1);
    let tile_len = genomes.len().div_ceil(tile_count);
    let mut tiles: Vec<(&[&Genome], &mut [DigestKey])> = genomes
        .chunks(tile_len)
        .zip(out.chunks_mut(tile_len))
        .collect();
    if pool::parallel_tasks(workers, "survey digests", &mut tiles, |_, (chunk, dst)| {
        compute_chunk(chunk, dst);
    })
    .is_err()
    {
        for (chunk, dst) in &mut tiles {
            compute_chunk(chunk, dst);
        }
    }
    out
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

fn lookup_row<'r, 'a>(rows: &'r [SurveyRow<'a>], id: OrganismId) -> Option<&'r SurveyRow<'a>> {
    rows.binary_search_by_key(&id, |row| row.id)
        .ok()
        .and_then(|idx| rows.get(idx))
}

fn evaluate_heredity_task(task: &HeredityTask<'_>, limits: &TraitLimits) -> HeredityOutcome {
    match *task {
        HeredityTask::FounderPair { previous, current } => {
            let (compared, identical) = compare(previous, current);
            let distance = trait_distance(previous, current, limits);
            HeredityOutcome::FounderPair {
                compared,
                identical,
                distance,
            }
        }
        HeredityTask::ParentPair {
            parent,
            child,
            grandparent,
        } => {
            let (compared, identical) = compare(parent, child);
            let distance = trait_distance(parent, child, limits);
            let traits_differ = distance > 0.0 || parent.traits() != child.traits();
            let triple = grandparent.map(|gp| {
                let (variants, carried_count) = carried(gp, parent, child);
                (variants, carried_count, trait_distance(gp, child, limits))
            });
            HeredityOutcome::ParentPair {
                compared,
                identical,
                distance,
                traits_differ,
                triple,
            }
        }
    }
}

fn heredity(rows: &[SurveyRow<'_>], limits: &TraitLimits, workers: usize) -> Heredity {
    let mut heredity = Heredity::default();
    let mut tasks: Vec<HeredityTask<'_>> = Vec::with_capacity(rows.len());
    let mut previous_founder: Option<&Genome> = None;
    for row in rows {
        let Some(genome) = row.genome else {
            continue;
        };
        let Some(parent_id) = row.parent else {
            if let Some(previous) = previous_founder
                && previous.topology() == genome.topology()
            {
                tasks.push(HeredityTask::FounderPair {
                    previous,
                    current: genome,
                });
            }
            previous_founder = Some(genome);
            continue;
        };
        let Some(parent_row) = lookup_row(rows, parent_id) else {
            continue;
        };
        let Some(parent) = parent_row.genome else {
            continue;
        };
        if parent.topology() != genome.topology() {
            heredity.pairs_across_topologies += 1;
            continue;
        }
        let grandparent = parent_row
            .parent
            .and_then(|gp_id| lookup_row(rows, gp_id))
            .and_then(|gp| gp.genome)
            .filter(|gp| gp.topology() == genome.topology());
        tasks.push(HeredityTask::ParentPair {
            parent,
            child: genome,
            grandparent,
        });
    }

    let mut outcomes = vec![
        HeredityOutcome::FounderPair {
            compared: 0,
            identical: 0,
            distance: 0.0,
        };
        tasks.len()
    ];
    if workers <= 1 || tasks.len() < 64 {
        for (task, dst) in tasks.iter().zip(outcomes.iter_mut()) {
            *dst = evaluate_heredity_task(task, limits);
        }
    } else {
        let tile_count = workers
            .saturating_mul(4)
            .min(64)
            .min(tasks.len().div_ceil(32))
            .max(1);
        let tile_len = tasks.len().div_ceil(tile_count);
        let mut tiles: Vec<(&[HeredityTask<'_>], &mut [HeredityOutcome])> = tasks
            .chunks(tile_len)
            .zip(outcomes.chunks_mut(tile_len))
            .collect();
        if pool::parallel_tasks(workers, "survey heredity", &mut tiles, |_, (chunk, dst)| {
            for (task, slot) in chunk.iter().zip(dst.iter_mut()) {
                *slot = evaluate_heredity_task(task, limits);
            }
        })
        .is_err()
        {
            for (chunk, dst) in &mut tiles {
                for (task, slot) in chunk.iter().zip(dst.iter_mut()) {
                    *slot = evaluate_heredity_task(task, limits);
                }
            }
        }
    }

    // Fold every counter and floating-point mean in exact ascending identity
    // order so output bits never depend on `workers`.
    let mut pair_traits = Mean::default();
    let mut founder_traits = Mean::default();
    let mut to_parent = Mean::default();
    let mut to_grandparent = Mean::default();
    for outcome in outcomes {
        match outcome {
            HeredityOutcome::FounderPair {
                compared,
                identical,
                distance,
            } => {
                heredity.founder_pairs += 1;
                heredity.founder_parameters += compared;
                heredity.founder_parameters_identical += identical;
                founder_traits.add(distance);
            }
            HeredityOutcome::ParentPair {
                compared,
                identical,
                distance,
                traits_differ,
                triple,
            } => {
                heredity.pairs += 1;
                heredity.pair_parameters += compared;
                heredity.pair_parameters_identical += identical;
                heredity.pairs_differing += u64::from(identical < compared || traits_differ);
                heredity.most_parameter_differences = heredity
                    .most_parameter_differences
                    .max(compared - identical);
                pair_traits.add(distance);
                if let Some((variants, carried_count, gp_distance)) = triple {
                    heredity.triples += 1;
                    heredity.parental_parameter_variants += variants;
                    heredity.carried_parameter_variants += carried_count;
                    to_parent.add(distance);
                    to_grandparent.add(gp_distance);
                }
            }
        }
    }
    heredity.pair_trait_distance = pair_traits.get();
    heredity.founder_trait_distance = founder_traits.get();
    heredity.triple_trait_distance_to_parent = to_parent.get();
    heredity.triple_trait_distance_to_grandparent = to_grandparent.get();
    heredity
}

/// `(parameters, bit-identical parameters)` between two genomes of one
/// topology.
fn compare(a: &Genome, b: &Genome) -> (u64, u64) {
    match (a.parameters(), b.parameters()) {
        (Weights::F32(xs), Weights::F32(ys)) => {
            let n = xs.len().min(ys.len());
            let same = xs[..n]
                .iter()
                .zip(&ys[..n])
                .filter(|(x, y)| x.to_bits() == y.to_bits())
                .count();
            (count(n), count(same))
        }
        (Weights::F16(xs), Weights::F16(ys)) => {
            let n = xs.len().min(ys.len());
            let same = xs[..n]
                .iter()
                .zip(&ys[..n])
                .filter(|(x, y)| x.to_bits() == y.to_bits())
                .count();
            (count(n), count(same))
        }
        _ => a
            .parameters()
            .bits()
            .zip(b.parameters().bits())
            .fold((0, 0), |(total, same), (x, y)| {
                (total + 1, same + u64::from(x == y))
            }),
    }
}

/// `(parameters where parent ≠ grandparent, of those where child = parent)`.
fn carried(grandparent: &Genome, parent: &Genome, child: &Genome) -> (u64, u64) {
    match (
        grandparent.parameters(),
        parent.parameters(),
        child.parameters(),
    ) {
        (Weights::F32(gs), Weights::F32(ps), Weights::F32(cs)) => {
            let n = gs.len().min(ps.len()).min(cs.len());
            let (mut variants, mut kept) = (0_usize, 0_usize);
            for ((g, p), c) in gs[..n].iter().zip(&ps[..n]).zip(&cs[..n]) {
                let pb = p.to_bits();
                if g.to_bits() != pb {
                    variants += 1;
                    kept += usize::from(pb == c.to_bits());
                }
            }
            (count(variants), count(kept))
        }
        (Weights::F16(gs), Weights::F16(ps), Weights::F16(cs)) => {
            let n = gs.len().min(ps.len()).min(cs.len());
            let (mut variants, mut kept) = (0_usize, 0_usize);
            for ((g, p), c) in gs[..n].iter().zip(&ps[..n]).zip(&cs[..n]) {
                let pb = p.to_bits();
                if g.to_bits() != pb {
                    variants += 1;
                    kept += usize::from(pb == c.to_bits());
                }
            }
            (count(variants), count(kept))
        }
        _ => grandparent
            .parameters()
            .bits()
            .zip(parent.parameters().bits())
            .zip(child.parameters().bits())
            .filter(|((g, p), _)| g != p)
            .fold((0, 0), |(variants, carried), ((_, p), c)| {
                (variants + 1, carried + u64::from(p == c))
            }),
    }
}

/// The baseline trait terms: affinity, motion response, then
/// `ln(sensing scale)` by the host-independent [`numerics::ln`], so every
/// survey is identical on every host. Active `field-ecology-v2` heritable
/// traits (`pcp-polarity` and `cadherin-4d`) follow in canonical order.
fn baseline_trait_vector(genome: &Genome) -> impl Iterator<Item = f64> + '_ {
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

/// The Euclidean distance between two genomes' trait vectors, each block in
/// units of its norm budget: affinity over `affinity_norm_limit`, motion
/// response over `motion_response_norm_limit`, `ln(sensing scale)` as is,
/// optional `pcp-polarity` strength over `1` and wedge curvature over
/// `maximum_wedge_curvature` (or `1` when zero), and optional `cadherin-4d`
/// unit tag and cohesiveness over `1`. It is dimensionless, so it compares
/// across configurations whatever their motility gain. Summed in vector order.
fn trait_distance(a: &Genome, b: &Genome, limits: &TraitLimits) -> f64 {
    let affinity = AFFINITY_TERMS;
    let motion = affinity + MOTION_RESPONSE_TERMS;
    let mut sum: f64 = baseline_trait_vector(a)
        .zip(baseline_trait_vector(b))
        .enumerate()
        .map(|(index, (x, y))| {
            let budget = if index < affinity {
                limits.affinity_norm()
            } else if index < motion {
                limits.motion_response_norm()
            } else {
                1.0
            };
            let difference = (x - y) / budget;
            difference * difference
        })
        .sum();
    if let (Some(pa), Some(pb)) = (a.traits().polarity_traits(), b.traits().polarity_traits()) {
        let strength = pa.polarity_strength() - pb.polarity_strength();
        sum += strength * strength;
        let wedge_budget = if limits.maximum_wedge_curvature() > 0.0 {
            limits.maximum_wedge_curvature()
        } else {
            1.0
        };
        let wedge = (pa.wedge_curvature() - pb.wedge_curvature()) / wedge_budget;
        sum += wedge * wedge;
    }
    if let (Some(ca), Some(cb)) = (a.traits().cadherin_traits(), b.traits().cadherin_traits()) {
        for (&x, &y) in ca.tag().iter().zip(cb.tag()) {
            let diff = x - y;
            sum += diff * diff;
        }
        let cohesion = ca.cohesiveness() - cb.cohesiveness();
        sum += cohesion * cohesion;
    }
    sum.sqrt()
}

/// The SHA-256 of a genome's parameters' bits, each little-endian in the
/// run's weight dtype (four bytes for F32, two for binary16), reusing `bytes`
/// to avoid per-genome heap allocation.
fn parameter_digest_into(genome: &Genome, bytes: &mut Vec<u8>) -> Sha256Digest {
    bytes.clear();
    match genome.parameters() {
        Weights::F32(values) => {
            bytes.reserve(values.len().saturating_mul(4));
            for value in values {
                bytes.extend_from_slice(&value.to_bits().to_le_bytes());
            }
        }
        Weights::F16(values) => {
            bytes.reserve(values.len().saturating_mul(2));
            for value in values {
                bytes.extend_from_slice(&value.to_bits().to_le_bytes());
            }
        }
    }
    Sha256Digest::of(bytes)
}

/// The SHA-256 of a genome's trait vector's bits, reusing `bytes`.
fn trait_digest_into(genome: &Genome, bytes: &mut Vec<u8>) -> Sha256Digest {
    bytes.clear();
    bytes.reserve(8 * 80);
    for value in baseline_trait_vector(genome) {
        bytes.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    if let Some(polarity) = genome.traits().polarity_traits() {
        bytes.extend_from_slice(&polarity.polarity_strength().to_bits().to_le_bytes());
        bytes.extend_from_slice(&polarity.wedge_curvature().to_bits().to_le_bytes());
    }
    if let Some(cadherin) = genome.traits().cadherin_traits() {
        for &component in cadherin.tag() {
            bytes.extend_from_slice(&component.to_bits().to_le_bytes());
        }
        bytes.extend_from_slice(&cadherin.cohesiveness().to_bits().to_le_bytes());
    }
    Sha256Digest::of(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flife_core::ValidatedConfig;
    use flife_engine::{Arrangement, NumericalProfile, Stepped, World};

    const STARTER: &str = include_str!("../../../presets/reference-small-world.toml");

    fn opt_f64_bits(value: Option<f64>) -> Option<u64> {
        value.map(f64::to_bits)
    }

    #[test]
    fn parallel_survey_matches_serial_bit_for_bit() {
        for dtype in ["f32", "f16"] {
            let mut toml = STARTER.to_owned();
            for (from, to) in [
                ("width = 128.0\n", "width = 64.0\n"),
                ("height = 128.0\n", "height = 64.0\n"),
                ("grid_width = 128\n", "grid_width = 64\n"),
                ("grid_height = 128\n", "grid_height = 64\n"),
                ("initial_count = 4096\n", "initial_count = 256\n"),
                ("maximum_count = 16384\n", "maximum_count = 2048\n"),
                ("initial_energy = 2.0\n", "initial_energy = 4.0\n"),
                (
                    "maximum_attempt_rate_per_second = 1.0\n",
                    "maximum_attempt_rate_per_second = 40.0\n",
                ),
                (
                    "minimum_parent_energy_before_attempt = 3.0\n",
                    "minimum_parent_energy_before_attempt = 1.2\n",
                ),
                ("cooldown_seconds = 0.5\n", "cooldown_seconds = 0.02\n"),
            ] {
                toml = toml.replace(from, to);
            }
            if dtype == "f16" {
                toml = toml.replace(
                    "canonical_weight_dtype = \"f32\"",
                    "canonical_weight_dtype = \"f16\"",
                );
            }
            let validated = ValidatedConfig::from_toml(&toml).expect("valid config");
            let mut world = World::found(
                &validated,
                Arrangement::Compact,
                NumericalProfile::reference_f32().expect("profile"),
            )
            .expect("founded world");
            for _ in 0..32 {
                let stepped = world.step().expect("step");
                assert!(matches!(stepped, Stepped::Published(_)));
            }
            let serial = Survey::of_with_workers(&world, 1);
            assert!(
                serial.heredity.founder_pairs > 0
                    && serial.heredity.pairs > 0
                    && serial.heredity.triples > 0,
                "{dtype} test world must exercise founder pairs, parent-child pairs, and grandparent triples: {:?}",
                serial.heredity
            );
            for workers in [2_usize, 4, 8] {
                let parallel = Survey::of_with_workers(&world, workers);
                assert_eq!(parallel.tick, serial.tick);
                assert_eq!(parallel.live, serial.live);
                assert_eq!(parallel.founders_alive, serial.founders_alive);
                assert_eq!(parallel.descendants_alive, serial.descendants_alive);
                assert_eq!(parallel.max_generation, serial.max_generation);
                assert_eq!(parallel.generations, serial.generations);
                assert_eq!(parallel.lineages_alive, serial.lineages_alive);
                assert_eq!(parallel.identities_issued, serial.identities_issued);
                assert_eq!(parallel.distinct_genomes, serial.distinct_genomes);
                assert_eq!(
                    parallel.distinct_parameter_sets,
                    serial.distinct_parameter_sets
                );
                assert_eq!(
                    parallel.distinct_trait_vectors,
                    serial.distinct_trait_vectors
                );
                assert_eq!(parallel.topologies, serial.topologies);
                assert_eq!(parallel.heredity.pairs, serial.heredity.pairs);
                assert_eq!(
                    parallel.heredity.pairs_across_topologies,
                    serial.heredity.pairs_across_topologies
                );
                assert_eq!(
                    parallel.heredity.pair_parameters,
                    serial.heredity.pair_parameters
                );
                assert_eq!(
                    parallel.heredity.pair_parameters_identical,
                    serial.heredity.pair_parameters_identical
                );
                assert_eq!(
                    parallel.heredity.pairs_differing,
                    serial.heredity.pairs_differing
                );
                assert_eq!(
                    parallel.heredity.most_parameter_differences,
                    serial.heredity.most_parameter_differences
                );
                assert_eq!(
                    opt_f64_bits(parallel.heredity.pair_trait_distance),
                    opt_f64_bits(serial.heredity.pair_trait_distance)
                );
                assert_eq!(
                    parallel.heredity.founder_pairs,
                    serial.heredity.founder_pairs
                );
                assert_eq!(
                    parallel.heredity.founder_parameters,
                    serial.heredity.founder_parameters
                );
                assert_eq!(
                    parallel.heredity.founder_parameters_identical,
                    serial.heredity.founder_parameters_identical
                );
                assert_eq!(
                    opt_f64_bits(parallel.heredity.founder_trait_distance),
                    opt_f64_bits(serial.heredity.founder_trait_distance)
                );
                assert_eq!(parallel.heredity.triples, serial.heredity.triples);
                assert_eq!(
                    parallel.heredity.parental_parameter_variants,
                    serial.heredity.parental_parameter_variants
                );
                assert_eq!(
                    parallel.heredity.carried_parameter_variants,
                    serial.heredity.carried_parameter_variants
                );
                assert_eq!(
                    opt_f64_bits(parallel.heredity.triple_trait_distance_to_parent),
                    opt_f64_bits(serial.heredity.triple_trait_distance_to_parent)
                );
                assert_eq!(
                    opt_f64_bits(parallel.heredity.triple_trait_distance_to_grandparent),
                    opt_f64_bits(serial.heredity.triple_trait_distance_to_grandparent)
                );
            }
        }
    }

    #[test]
    fn v2_polarity_and_cadherin_traits_affect_digest_and_distance() {
        use flife_core::ids::GenomeId;
        use flife_genome::{CadherinTraits, GenomeQuota, PolarityTraits, TopologyCatalog, Traits};

        let bundle_a = include_str!("../../../presets/morphogenesis-tissue-v2.toml")
            .replacen("initial_count = 10954", "initial_count = 16", 1)
            .replacen("maximum_count = 262144", "maximum_count = 64", 1);
        let validated = ValidatedConfig::from_toml(&bundle_a).expect("Bundle A valid");
        let catalog = TopologyCatalog::from_config(&validated).expect("catalog");
        let limits = TraitLimits::from_config(&validated).expect("limits");
        assert!(limits.polarity_enabled());
        assert!(limits.cadherin_enabled());
        let mut quota = GenomeQuota::from_config(&validated).expect("quota");
        let topology = catalog.default_topology();

        let mut affinity = [0.0; AFFINITY_TERMS];
        affinity[0] = 0.5;
        let motion = [0.0; MOTION_RESPONSE_TERMS];
        let base_pol = PolarityTraits::new(0.4, 0.05, limits.maximum_wedge_curvature())
            .expect("base polarity");
        let base_cad = CadherinTraits::new([1.0, 0.0, 0.0, 0.0], 0.5).expect("base cadherin");

        let mut seal = |id: u64, pol: PolarityTraits, cad: CadherinTraits| -> Genome {
            let traits =
                Traits::with_v2_traits(affinity, motion, 1.0, Some(pol), Some(cad), &limits)
                    .expect("valid v2 traits");
            let buf = quota.allocate(&catalog, topology).expect("alloc");
            Genome::seal(buf, GenomeId::new(id), &catalog, traits, None, &limits)
                .expect("seal genome")
        };

        let base = seal(1, base_pol, base_cad);
        let pol_strength = seal(
            2,
            PolarityTraits::new(0.7, 0.05, limits.maximum_wedge_curvature()).expect("pol"),
            base_cad,
        );
        let pol_wedge = seal(
            3,
            PolarityTraits::new(0.4, -0.05, limits.maximum_wedge_curvature()).expect("wedge"),
            base_cad,
        );
        let cad_tag = seal(
            4,
            base_pol,
            CadherinTraits::new([0.0, 1.0, 0.0, 0.0], 0.5).expect("cad tag"),
        );
        let cad_coh = seal(
            5,
            base_pol,
            CadherinTraits::new([1.0, 0.0, 0.0, 0.0], 0.9).expect("cad coh"),
        );

        let mut scratch = Vec::new();
        let d_base = trait_digest_into(&base, &mut scratch);
        for (label, other, expected_dist) in [
            ("polarity_strength", &pol_strength, 0.3),
            (
                "wedge_curvature",
                &pol_wedge,
                0.10 / limits.maximum_wedge_curvature(),
            ),
            ("cadherin_tag", &cad_tag, 2.0_f64.sqrt()),
            ("cohesiveness", &cad_coh, 0.4),
        ] {
            assert_ne!(
                trait_digest_into(other, &mut scratch),
                d_base,
                "{label} mutation must change trait digest"
            );
            let dist = trait_distance(&base, other, &limits);
            assert!(
                (dist - expected_dist).abs() < 1e-12,
                "{label}: got {dist}, expected {expected_dist}"
            );
        }

        // Zero `maximum_wedge_curvature` must never divide by zero or yield NaN.
        let zero_wedge_limits = TraitLimits::new(
            limits.affinity_norm(),
            limits.motion_response_norm(),
            limits.scale_range(),
            4.0,
        )
        .expect("base limits")
        .with_polarity(0.8, 0.0, 0.02)
        .expect("zero-wedge polarity")
        .with_cadherin(0.02)
        .expect("cadherin");
        let zw_pol = PolarityTraits::new(0.4, 0.0, 0.0).expect("zw pol");
        let zw_a = seal(6, zw_pol, base_cad);
        let zw_b = seal(7, zw_pol, base_cad);
        assert_eq!(trait_distance(&zw_a, &zw_b, &zero_wedge_limits), 0.0);

        for genome in [base, pol_strength, pol_wedge, cad_tag, cad_coh, zw_a, zw_b] {
            quota.retire(genome).expect("retire");
        }
    }

    #[test]
    fn parallel_v2_bundle_a_survey_matches_serial_bit_for_bit() {
        let mut toml = include_str!("../../../presets/morphogenesis-tissue-v2.toml").to_owned();
        for (from, to) in [
            ("initial_count = 10954\n", "initial_count = 128\n"),
            ("maximum_count = 262144\n", "maximum_count = 1024\n"),
            ("initial_energy = 2.0\n", "initial_energy = 4.0\n"),
            (
                "maximum_attempt_rate_per_second = 1.0\n",
                "maximum_attempt_rate_per_second = 40.0\n",
            ),
            (
                "minimum_parent_energy_before_attempt = 3.0\n",
                "minimum_parent_energy_before_attempt = 1.2\n",
            ),
            ("cooldown_seconds = 0.5\n", "cooldown_seconds = 0.02\n"),
        ] {
            toml = toml.replace(from, to);
        }
        let validated = ValidatedConfig::from_toml(&toml).expect("valid Bundle A");
        let mut world = World::found(
            &validated,
            Arrangement::Compact,
            NumericalProfile::reference_f32().expect("profile"),
        )
        .expect("founded Bundle A");
        for _ in 0..24 {
            assert!(matches!(world.step().expect("step"), Stepped::Published(_)));
        }
        let serial = Survey::of_with_workers(&world, 1);
        assert!(
            serial.heredity.founder_pairs > 0 && serial.heredity.pairs > 0,
            "Bundle A world should have founder and parent-child pairs: {:?}",
            serial.heredity
        );
        for workers in [2_usize, 4, 8] {
            let parallel = Survey::of_with_workers(&world, workers);
            assert_eq!(parallel.distinct_genomes, serial.distinct_genomes);
            assert_eq!(
                parallel.distinct_trait_vectors,
                serial.distinct_trait_vectors
            );
            assert_eq!(
                parallel.heredity.pairs_differing,
                serial.heredity.pairs_differing
            );
            assert_eq!(
                opt_f64_bits(parallel.heredity.pair_trait_distance),
                opt_f64_bits(serial.heredity.pair_trait_distance)
            );
            assert_eq!(
                opt_f64_bits(parallel.heredity.founder_trait_distance),
                opt_f64_bits(serial.heredity.founder_trait_distance)
            );
        }
    }
}
