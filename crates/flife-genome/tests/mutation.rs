//! FL-010: founders and mutation proposals under `fnp-philox-semantic-v1`
//! (plan §7.2, §7.3, §16.2, A.3 items 1–4, Appendix C.16, INV-11).
//!
//! Seed and order invariance, fixed versus heritable components, finite
//! retries, and canonical rounding. The full-path fixtures take their
//! expected bits from an oracle script that uses NumPy 2.5.4's `Philox`,
//! Python `Decimal` at 80 digits and `numpy.float32` rounding, and imports
//! no FrankenLife or FrankenNumPy code.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use flife_core::binary16::F16;
use flife_core::ids::{GenomeId, OrganismId};
use flife_core::numerics::exp;
use flife_core::rng::{Address, Lane, LaneKind, SemanticRng, Stream};
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_genome::{
    BirthAttempt, Genome, GenomeQuota, MAXIMUM_CANDIDATES, MutationParameters, Mutator,
    ParameterValues, Prepared, Proposal, RejectionReason, TopologyCatalog, TopologyId, TraitLimits,
    Traits, Transition, WeightDtype, Weights, WeightsMut, fan_in_weight, founder, initial_scale,
    map_parameters, perturb, perturb_weight, reflect_log_scale,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn appendix_a() -> ValidatedConfig {
    let appendix = &PLAN[PLAN.find("## Appendix A").expect("Appendix A heading")..];
    let start = appendix.find("```toml\n").expect("Appendix A TOML block") + "```toml\n".len();
    let length = appendix[start..].find("```").expect("closing fence");
    ValidatedConfig::from_toml(&appendix[start..start + length]).expect("Appendix A is valid")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn catalog() -> TopologyCatalog {
    TopologyCatalog::from_config(&appendix_a()).expect("Appendix A catalog")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn limits() -> TraitLimits {
    TraitLimits::from_config(&appendix_a()).expect("Appendix A limits")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn quota() -> GenomeQuota {
    GenomeQuota::new(1 << 32, 1 << 32).expect("quotas in order")
}

/// Appendix A's rates with every operator off.
fn quiet() -> MutationParameters {
    MutationParameters {
        topology_change_probability: 0.0,
        weight_probability: 0.0,
        weight_standard_deviation: 0.0,
        replacement_probability: 0.0,
        chemistry_standard_deviation: 0.0,
        sensing_log_scale_standard_deviation: 0.0,
        reject_effective_noop: false,
        maximum_candidates: MAXIMUM_CANDIDATES,
    }
}

const SEED: u64 = 42;

fn attempt(parent: u64, tick: u64) -> BirthAttempt {
    BirthAttempt {
        parent: OrganismId::new(parent),
        tick: Tick::new(tick),
    }
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn founder_of(
    rng: &SemanticRng,
    organism: u64,
    catalog: &TopologyCatalog,
    quota: &mut GenomeQuota,
) -> Genome {
    founder(
        rng,
        OrganismId::new(organism),
        GenomeId::new(organism),
        catalog,
        quota,
        &limits(),
        initial_scale(&appendix_a()),
    )
    .expect("a founder")
}

/// A parent of the default member whose every parameter is `value`, in the
/// quota's weight dtype (`value` must be exact in it).
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn uniform_parent(value: f32, catalog: &TopologyCatalog, quota: &mut GenomeQuota) -> Genome {
    let rng = SemanticRng::new(SEED);
    let traits = *founder_of(&rng, 99, catalog, &mut quota.clone()).traits();
    let mut buffer = quota
        .allocate(catalog, catalog.default_topology())
        .expect("room for the parent");
    match buffer.values_mut() {
        WeightsMut::F32(values) => values.fill(value),
        WeightsMut::F16(values) => {
            let half = F16::from_f64(f64::from(value));
            assert_eq!(half.to_f32(), value, "exact in binary16");
            values.fill(half);
        }
    }
    Genome::seal(
        buffer,
        GenomeId::new(1_000),
        catalog,
        traits,
        None,
        &limits(),
    )
    .expect("an in-bound parent")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn mutator(parameters: MutationParameters, catalog: &TopologyCatalog) -> Mutator<'_> {
    Mutator::new(SemanticRng::new(SEED), parameters, catalog, limits()).expect("valid rates")
}

#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn child(
    mutator: &Mutator<'_>,
    parent: &Genome,
    at: BirthAttempt,
    quota: &mut GenomeQuota,
) -> (Box<Genome>, flife_genome::MutationRecord) {
    match mutator
        .propose(parent, at, GenomeId::new(5_000), quota)
        .expect("no capacity error")
    {
        Proposal::Child { genome, record } => (genome, record),
        Proposal::Rejected { candidates } => panic!("birth rejected: {candidates:?}"),
    }
}

/// Everything heritable, bit for bit.
fn content(genome: &Genome) -> (TopologyId, Vec<u32>, Vec<u64>) {
    let traits = genome.traits();
    let mut values: Vec<u64> = traits.affinity().iter().map(|v| v.to_bits()).collect();
    values.extend(traits.motion_response().iter().map(|v| v.to_bits()));
    values.push(traits.sensing_scale().to_bits());
    (
        genome.topology(),
        genome.parameters().iter().map(|v| v.to_bits()).collect(),
        values,
    )
}

/// An F32 genome's parameters.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn f32s(genome: &Genome) -> &[f32] {
    genome.parameters().as_f32().expect("an F32 genome")
}

fn squared(values: &[f64]) -> f64 {
    values
        .iter()
        .fold(0.0, |total, &value| total + value * value)
}

#[test]
fn founders_are_functions_of_the_seed_and_their_identity() {
    let catalog = catalog();
    let rng = SemanticRng::new(SEED);
    let (mut forward, mut backward) = (quota(), quota());
    let ahead: BTreeMap<u64, _> = (1..=16)
        .map(|organism| {
            let genome = founder_of(&rng, organism, &catalog, &mut forward);
            (organism, content(&genome))
        })
        .collect();
    let behind: BTreeMap<u64, _> = (1..=16)
        .rev()
        .map(|organism| {
            let genome = founder_of(&rng, organism, &catalog, &mut backward);
            (organism, content(&genome))
        })
        .collect();
    assert_eq!(ahead, behind);
    assert_eq!(forward, backward);

    // Organism 1's weight (layer 0, output 3, input 101) is
    // `fan-in-scaled-symmetric-v1` of NumPy's word 0x7cf8385e9e0737d2 at
    // fan-in 104, rounded by numpy.float32.
    let one = founder_of(&rng, 1, &catalog, &mut quota());
    let topology = catalog
        .topology(catalog.default_topology())
        .expect("default member");
    let layers = topology.layers();
    let offset = layers[0].weight(3, 101).expect("in range");
    assert_eq!(f32s(&one)[offset].to_bits(), 0xbb83_c278);
    for layer in layers {
        for output in 0..layer.outputs {
            let bias = layer.bias(output).expect("in range");
            assert_eq!(f32s(&one)[bias].to_bits(), 0, "biases start at zero");
        }
    }

    // Distinct organisms and another seed draw distinct founders.
    let two = founder_of(&rng, 2, &catalog, &mut quota());
    let other = founder_of(&SemanticRng::new(SEED + 1), 1, &catalog, &mut quota());
    assert_ne!(content(&one).1, content(&two).1);
    assert_ne!(content(&one).1, content(&other).1);
    assert_ne!(content(&one).2, content(&two).2);

    // Traits: unit affinity, a motion response on its norm sphere, the
    // configured scale; isotropy to sampling error over 512 founders.
    let limits = limits();
    let motion_limit = limits.motion_response_norm();
    let mut mean = [0.0_f64; 32];
    let mut square = 0.0;
    let count = 512;
    for organism in 1..=count {
        let genome = founder_of(&rng, organism, &catalog, &mut quota());
        let traits = genome.traits();
        let affinity = squared(traits.affinity());
        assert!(
            affinity <= 1.0 && affinity > 1.0 - 8.0 * f64::EPSILON,
            "{affinity}"
        );
        let motion = squared(traits.motion_response());
        assert!(motion <= motion_limit * motion_limit);
        assert!(motion > motion_limit * motion_limit * (1.0 - 8.0 * f64::EPSILON));
        assert_eq!(traits.sensing_scale(), initial_scale(&appendix_a()));
        for (total, &value) in mean.iter_mut().zip(traits.affinity()) {
            *total += value;
        }
        square += traits.affinity()[7] * traits.affinity()[7];
    }
    let n = count as f64;
    // Each component has mean 0 and variance 1/32.
    let standard_error = (1.0 / 32.0 / n).sqrt();
    for total in mean {
        assert!((total / n).abs() < 5.0 * standard_error, "{}", total / n);
    }
    assert!((square / n - 1.0 / 32.0).abs() < 0.01, "{}", square / n);
}

#[test]
fn zero_rates_give_a_bit_identical_child() {
    let catalog = catalog();
    let mut quota = quota();
    let parent = founder_of(&SemanticRng::new(SEED), 3, &catalog, &mut quota);
    let mutator = mutator(quiet(), &catalog);
    let (genome, record) = child(&mutator, &parent, attempt(3, 10), &mut quota);
    assert_eq!(content(&genome), content(&parent));
    assert_eq!(genome.parent(), Some(parent.id()));
    assert!(
        !genome
            .shared_parameters()
            .same_buffer(&parent.shared_parameters())
    );
    assert_eq!(
        (
            record.candidate,
            record.topology,
            record.replaced,
            record.perturbed
        ),
        (0, None, 0, 0)
    );
    assert!(record.rejected.is_empty());
}

#[test]
fn each_operator_changes_only_its_own_heritable_component() {
    let catalog = catalog();
    let mut quota = quota();
    let parent = founder_of(&SemanticRng::new(SEED), 4, &catalog, &mut quota);
    let (topology, parameters, traits) = content(&parent);
    let count = parameters.len();
    let at = attempt(4, 77);
    let traits_of = |genome: &Genome| -> Traits { *genome.traits() };

    // Weights and biases only.
    let rates = MutationParameters {
        weight_probability: 1.0,
        weight_standard_deviation: 0.02,
        ..quiet()
    };
    let (genome, record) = child(&mutator(rates, &catalog), &parent, at, &mut quota);
    assert_eq!(record.perturbed as usize, count);
    let (t, p, r) = content(&genome);
    assert_eq!((t, &r), (topology, &traits));
    let changed = p.iter().zip(&parameters).filter(|(a, b)| a != b).count();
    assert!(changed + 2 >= count, "{changed} of {count}");

    // Replacement only: every parameter is the initializer of its own word 1.
    let rates = MutationParameters {
        replacement_probability: 1.0,
        ..quiet()
    };
    let (genome, record) = child(&mutator(rates, &catalog), &parent, at, &mut quota);
    assert_eq!(record.replaced as usize, count);
    assert_eq!(traits_of(&genome), traits_of(&parent));
    let rng = SemanticRng::new(SEED);
    let shape = catalog.topology(topology).expect("parent member").layers()[1];
    for (kind, input, offset) in [
        (LaneKind::Weight, 17, shape.weight(5, 17)),
        (LaneKind::Bias, 0, shape.bias(5)),
    ] {
        let lane =
            Lane::new(0, kind, Lane::neuron_index(1, 5, input).expect("neuron")).expect("lane");
        let word = rng.block(Address {
            stream: Stream::Mutation,
            entity: 4,
            ordinal: 77,
            lane,
            draw: 0,
        })[1];
        let expected = fan_in_weight(word, shape.inputs).expect("fan-in");
        assert_eq!(f32s(&genome)[offset.expect("in range")], expected);
    }

    // Affinity and motion response only.
    let rates = MutationParameters {
        chemistry_standard_deviation: 0.05,
        ..quiet()
    };
    let (genome, _) = child(&mutator(rates, &catalog), &parent, at, &mut quota);
    let (t, p, _) = content(&genome);
    assert_eq!((t, &p), (topology, &parameters));
    let ours = traits_of(&genome);
    let theirs = traits_of(&parent);
    assert_ne!(ours.affinity(), theirs.affinity());
    assert_ne!(ours.motion_response(), theirs.motion_response());
    assert_eq!(ours.sensing_scale(), theirs.sensing_scale());
    let affinity = squared(ours.affinity());
    assert!(affinity <= 1.0 && affinity > 1.0 - 8.0 * f64::EPSILON);
    let limit = limits().motion_response_norm();
    assert!(squared(ours.motion_response()) <= limit * limit);

    // Sensing scale only.
    let rates = MutationParameters {
        sensing_log_scale_standard_deviation: 0.02,
        ..quiet()
    };
    let (genome, _) = child(&mutator(rates, &catalog), &parent, at, &mut quota);
    let ours = traits_of(&genome);
    assert_eq!(content(&genome).1, parameters);
    assert_eq!(ours.affinity(), theirs.affinity());
    assert_eq!(ours.motion_response(), theirs.motion_response());
    assert_ne!(ours.sensing_scale(), theirs.sensing_scale());
    let [low, high] = limits().scale_range();
    assert!((low..=high).contains(&ours.sensing_scale()));

    // Topology only: the child is the declared map of the parent, with the
    // map's words from its own lanes.
    let rates = MutationParameters {
        topology_change_probability: 1.0,
        ..quiet()
    };
    let (genome, record) = child(&mutator(rates, &catalog), &parent, at, &mut quota);
    let change = record.topology.expect("a topology change");
    assert_eq!((change.from, change.to), (topology, genome.topology()));
    assert_ne!(change.to, topology);
    assert_eq!(traits_of(&genome), traits_of(&parent));
    let source = catalog.topology(change.from).expect("source");
    let target = catalog.topology(change.to).expect("target");
    assert_eq!(
        Transition::between(source, target).expect("a supported change"),
        change.transition
    );
    let mut expected = vec![f32::NAN; target.parameter_count()];
    map_parameters(source, f32s(&parent), target, &mut expected, |weight| {
        let index = Lane::neuron_index(weight.layer, weight.output, weight.input).expect("neuron");
        rng.block(Address {
            stream: Stream::Mutation,
            entity: 4,
            ordinal: 77,
            lane: Lane::new(0, LaneKind::MapWeight, index).expect("lane"),
            draw: 0,
        })[0]
    })
    .expect("the map applies");
    let bits = |values: &[f32]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(f32s(&genome)), bits(&expected));
}

#[test]
fn proposals_depend_only_on_parent_tick_and_candidate() {
    let catalog = catalog();
    let rng = SemanticRng::new(SEED);
    let rates = MutationParameters::from_config(&appendix_a()).expect("Appendix A rates");
    let rates = MutationParameters {
        // Enough change per child to make any order effect visible.
        weight_probability: 0.05,
        topology_change_probability: 0.3,
        ..rates
    };
    let mutator = mutator(rates, &catalog);
    let parents: Vec<Genome> = (1..=12)
        .map(|organism| founder_of(&rng, organism, &catalog, &mut quota()))
        .collect();
    let run = |order: &[usize]| -> BTreeMap<u64, _> {
        let mut quota = quota();
        order
            .iter()
            .map(|&index| {
                let parent = &parents[index];
                let organism = parent.id().get();
                let (genome, record) = child(&mutator, parent, attempt(organism, 500), &mut quota);
                (organism, (content(&genome), record))
            })
            .collect()
    };
    let forward: Vec<usize> = (0..parents.len()).collect();
    let backward: Vec<usize> = forward.iter().rev().copied().collect();
    let interleaved: Vec<usize> = forward
        .iter()
        .step_by(2)
        .chain(forward.iter().skip(1).step_by(2))
        .copied()
        .collect();
    let reference = run(&forward);
    assert_eq!(run(&backward), reference);
    assert_eq!(run(&interleaved), reference);

    // Four workers, each with its own quota, as a scheduler might split them.
    let mut parallel = BTreeMap::new();
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|worker| {
                let (parents, mutator) = (&parents, &mutator);
                scope.spawn(move || {
                    let mut quota = quota();
                    parents
                        .iter()
                        .skip(worker)
                        .step_by(4)
                        .map(|parent| {
                            let organism = parent.id().get();
                            let (genome, record) =
                                child(mutator, parent, attempt(organism, 500), &mut quota);
                            (organism, (content(&genome), record))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for worker in workers {
            parallel.extend(worker.join().expect("worker finished"));
        }
    });
    assert_eq!(parallel, reference);

    // An operational retry of the same tick repeats its proposal; a later
    // tick and another seed draw fresh ones.
    let parent = &parents[0];
    let organism = parent.id().get();
    let mut quota = quota();
    let first = content(&child(&mutator, parent, attempt(organism, 500), &mut quota).0);
    let again = content(&child(&mutator, parent, attempt(organism, 500), &mut quota).0);
    let later = content(&child(&mutator, parent, attempt(organism, 501), &mut quota).0);
    assert_eq!(first, again);
    assert_ne!(first, later);
    let reseeded =
        Mutator::new(SemanticRng::new(SEED + 1), rates, &catalog, limits()).expect("valid rates");
    let other = content(&child(&reseeded, parent, attempt(organism, 500), &mut quota).0);
    assert_ne!(first, other);
}

#[test]
fn c16_exhausted_no_op_candidates_reject_the_birth_without_charge() {
    let catalog = catalog();
    let mut quota = quota();
    // Every parameter is 0.5, whose F32 half-ulp is 2^-25. A perturbation of
    // at most 1e-12 × 8.6 rounds back to the parent: every candidate is an
    // effective no-op after canonical rounding.
    let parent = uniform_parent(0.5, &catalog, &mut quota);
    let before = quota.clone();
    for maximum in [MAXIMUM_CANDIDATES, 3, 1] {
        let rates = MutationParameters {
            weight_probability: 1.0,
            weight_standard_deviation: 1.0e-12,
            reject_effective_noop: true,
            maximum_candidates: maximum,
            ..quiet()
        };
        let proposal = mutator(rates, &catalog)
            .propose(&parent, attempt(1, 9), GenomeId::new(2), &mut quota)
            .expect("no capacity error");
        let Proposal::Rejected { candidates } = proposal else {
            panic!("an effective no-op was accepted");
        };
        assert_eq!(candidates.len(), usize::from(maximum));
        for (index, rejection) in candidates.iter().enumerate() {
            assert_eq!(usize::from(rejection.candidate), index);
            assert_eq!(rejection.reason, RejectionReason::EffectiveNoop);
        }
        // Nothing leaked: logical, allocated and live counts are unchanged.
        assert_eq!(quota, before);
    }

    // Without the strict rule the same no-op is an exact clone.
    let rates = MutationParameters {
        weight_probability: 1.0,
        weight_standard_deviation: 1.0e-12,
        ..quiet()
    };
    let (genome, record) = child(
        &mutator(rates, &catalog),
        &parent,
        attempt(1, 9),
        &mut quota,
    );
    assert_eq!(content(&genome), content(&parent));
    assert_eq!(record.perturbed as usize, genome.parameters().len());
}

/// C.16 in compact F16 (FL-030, §24.2's mutation fidelity): binary16's half
/// step at 0.5 is 2^-12, so a perturbation of 1e-6 × N, at most about 8.6e-6,
/// always rounds back to the parent. The same rates change an F32 parent,
/// whose half-ulp there is 2^-25. Every F16 candidate is an effective no-op
/// and the birth is rejected without charge.
#[test]
fn c16_in_f16_a_step_below_half_a_binary16_step_is_a_no_op() {
    let catalog = catalog();
    let rates = MutationParameters {
        weight_probability: 1.0,
        weight_standard_deviation: 1.0e-6,
        reject_effective_noop: true,
        ..quiet()
    };

    let mut wide = quota();
    let parent = uniform_parent(0.5, &catalog, &mut wide);
    let (genome, record) = child(&mutator(rates, &catalog), &parent, attempt(1, 9), &mut wide);
    assert_ne!(content(&genome), content(&parent), "F32 resolves the step");
    assert_eq!(record.perturbed as usize, genome.parameters().len());

    let mut quota = quota().with_dtype(WeightDtype::F16);
    let parent = uniform_parent(0.5, &catalog, &mut quota);
    let before = quota.clone();
    let proposal = mutator(rates, &catalog)
        .propose(&parent, attempt(1, 9), GenomeId::new(2), &mut quota)
        .expect("no capacity error");
    let Proposal::Rejected { candidates } = proposal else {
        panic!("an effective binary16 no-op was accepted");
    };
    assert_eq!(candidates.len(), usize::from(MAXIMUM_CANDIDATES));
    assert!(
        candidates
            .iter()
            .all(|rejection| rejection.reason == RejectionReason::EffectiveNoop),
        "{candidates:?}"
    );
    assert_eq!(quota, before, "nothing is charged");
}

/// An F16 genome's parameters.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn f16s(genome: &Genome) -> &[F16] {
    genome.parameters().as_f16().expect("an F16 genome")
}

/// The binary16 steps just below and just above `value`'s magnitude.
fn binary16_steps(value: F16) -> (f64, f64) {
    let bits = value.to_bits() & 0x7FFF;
    let at = F16::from_bits(bits).to_f64();
    let above = F16::from_bits(bits + 1).to_f64() - at;
    let below = match bits {
        0 => above,
        _ => at - F16::from_bits(bits - 1).to_f64(),
    };
    (below, above)
}

/// Plan §24.2's F16 mutation fidelity under Appendix A's rates, with
/// topology changes off so that each child's parameters pair with its
/// parent's. 256 founders, each one F64 draw rounded once to F32 and once to
/// binary16, have 32 children each by the same attempts, and the operator
/// draws the same selections and steps in both dtypes. The children without
/// a replaced parameter (about e^-1 of them) are compared. Every candidate's
/// traits change, so the strict no-op rule rejects no more in F16 than in
/// F32. F16 changes no parameter that F32 leaves. It loses the perturbations
/// whose step rounds back to the binary16 parent, and their count is within
/// five standard deviations of the sum, over every perturbed parameter, of
/// the chance that a N(0, 0.02) step lies within half a binary16 step of it.
#[test]
fn f16_mutation_fidelity_under_appendix_a_rates() {
    let catalog = catalog();
    let rates = MutationParameters::from_config(&appendix_a()).expect("Appendix A rates");
    let deviation = rates.weight_standard_deviation;
    let rates = MutationParameters {
        topology_change_probability: 0.0,
        ..rates
    };
    let mutator = mutator(rates, &catalog);
    let rng = SemanticRng::new(SEED);
    let mut wide_quota = quota();
    let mut half_quota = quota().with_dtype(WeightDtype::F16);
    let (mut births, mut rejected) = (0_u64, [0_usize; 2]);
    let (mut kept, mut lost, mut expected) = (0_u64, 0_u64, 0.0_f64);
    // The squared step errors, and their round-to-nearest expectation: a
    // twelfth of the squared binary16 step at the child's value.
    let (mut distortion, mut uniform) = (0.0_f64, 0.0_f64);
    for organism in 1..=256 {
        let wide_parent = founder_of(&rng, organism, &catalog, &mut wide_quota);
        let half_parent = founder_of(&rng, organism, &catalog, &mut half_quota);
        for tick in 0..32 {
            let at = attempt(organism, tick);
            let (wide, wide_record) = child(&mutator, &wide_parent, at, &mut wide_quota.clone());
            let (half, half_record) = child(&mutator, &half_parent, at, &mut half_quota.clone());
            rejected[0] += wide_record.rejected.len();
            rejected[1] += half_record.rejected.len();
            assert_eq!(
                (wide_record.candidate, wide_record.perturbed),
                (half_record.candidate, half_record.perturbed),
                "the same selections in both dtypes"
            );
            if wide_record.replaced + half_record.replaced > 0 {
                continue;
            }
            births += 1;
            let pairs = f32s(&wide).iter().zip(f32s(&wide_parent));
            let halves = f16s(&half).iter().zip(f16s(&half_parent));
            for ((&wide_after, &wide_before), (&half_after, &half_before)) in pairs.zip(halves) {
                if wide_after == wide_before {
                    assert_eq!(half_after, half_before, "F16 changed what F32 kept");
                    continue;
                }
                let (below, above) = binary16_steps(half_before);
                expected += (below + above) / 2.0 / (deviation * (2.0 * PI).sqrt());
                if half_after == half_before {
                    lost += 1;
                } else {
                    kept += 1;
                    let wide_step = f64::from(wide_after) - f64::from(wide_before);
                    let half_step = half_after.to_f64() - half_before.to_f64();
                    distortion += ((half_step - wide_step) / deviation).powi(2);
                    let (below, above) = binary16_steps(half_after);
                    uniform += ((below + above) / 2.0 / deviation).powi(2) / 12.0;
                }
            }
        }
    }
    let rms = (distortion / kept as f64).sqrt();
    let nearest = (uniform / kept as f64).sqrt();
    println!(
        "{births} births: {} perturbed, F16 lost {lost} ({:.4}%) against {expected:.1} expected; \
         rejected candidates F32 {} F16 {}; F16 step error rms {rms:.3e} of the deviation \
         (round to nearest: {nearest:.3e})",
        kept + lost,
        100.0 * lost as f64 / (kept + lost) as f64,
        rejected[0],
        rejected[1],
    );
    assert!(births > 2_500, "{births}");
    assert_eq!(rejected[1], rejected[0]);
    assert!(lost > 0, "F16 lost no perturbation");
    assert!(
        (lost as f64 - expected).abs() < 5.0 * expected.sqrt() + 1.0,
        "{lost} lost against {expected} expected"
    );
    assert!(lost * 100 < kept + lost, "F16 lost 1% or more");
    // The realized steps carry round-to-nearest's error, no more.
    assert!(
        (rms / nearest - 1.0).abs() < 0.05,
        "{rms} against {nearest}"
    );
    assert!(rms < 0.01, "{rms}");
}

#[test]
fn invalid_candidates_are_rejected_never_reinterpreted() {
    let catalog = catalog();
    let mut quota = quota();
    let bound = limits().maximum_absolute_parameter() as f32;
    // At the bound, a unit perturbation of every parameter leaves it.
    let parent = uniform_parent(bound, &catalog, &mut quota);
    let before = quota.clone();
    let rates = MutationParameters {
        weight_probability: 1.0,
        weight_standard_deviation: 1.0,
        ..quiet()
    };
    let proposal = mutator(rates, &catalog)
        .propose(&parent, attempt(2, 3), GenomeId::new(2), &mut quota)
        .expect("no capacity error");
    let Proposal::Rejected { candidates } = proposal else {
        panic!("an out-of-bound child was accepted");
    };
    assert_eq!(candidates.len(), usize::from(MAXIMUM_CANDIDATES));
    assert!(candidates.iter().all(|rejection| matches!(
        rejection.reason,
        RejectionReason::Invalid(CoreError::NumericalInvalid { .. })
    )));
    assert_eq!(quota, before);

    // With one parameter at the bound and half of them perturbed, some
    // candidates fail and a later one succeeds; the record keeps the
    // failures in order.
    let mut buffer = quota
        .allocate(&catalog, catalog.default_topology())
        .expect("room");
    buffer.f32_mut().expect("an F32 buffer").fill(0.25);
    buffer.f32_mut().expect("an F32 buffer")[0] = bound;
    let traits = *parent.traits();
    let edge = Genome::seal(buffer, GenomeId::new(3), &catalog, traits, None, &limits())
        .expect("in bound");
    let rates = MutationParameters {
        weight_probability: 0.5,
        weight_standard_deviation: 0.5,
        ..quiet()
    };
    let mutator = mutator(rates, &catalog);
    let (tick, record) = (0..64)
        .find_map(|tick| {
            let (_, record) = child(&mutator, &edge, attempt(3, tick), &mut quota.clone());
            (record.candidate > 0).then_some((tick, record))
        })
        .expect("some attempt needs a retry");
    assert_eq!(record.rejected.len(), usize::from(record.candidate));
    for (index, rejection) in record.rejected.iter().enumerate() {
        assert_eq!(usize::from(rejection.candidate), index);
        assert!(matches!(rejection.reason, RejectionReason::Invalid(_)));
    }
    // The accepted candidate is in bound, and retrying the tick repeats it.
    let (genome, again) = child(&mutator, &edge, attempt(3, tick), &mut quota);
    assert_eq!(again, record);
    assert!(genome.parameters().iter().all(|value| value.abs() <= bound));
}

#[test]
fn canonical_rounding_fixtures() {
    // F64 arithmetic, rounded once to F32 with ties to even.
    let half_ulp = 2.0_f64.powi(-24);
    assert_eq!(perturb(1.0, half_ulp / 2.0), 1.0);
    assert_eq!(perturb(1.0, half_ulp), 1.0, "a tie rounds to even");
    assert_eq!(perturb(1.0, 1.5 * half_ulp), 1.0 + f32::EPSILON);
    assert_eq!(
        perturb(1.0 + f32::EPSILON, half_ulp),
        1.0 + 2.0 * f32::EPSILON
    );
    assert_eq!(perturb(0.0, 1.0e-12), 1.0e-12_f64 as f32);

    // Full path at parent 7, tick 1234, candidate 0, weight (1, 5, 17) of
    // the default member, against the independent oracle:
    // - replacement: fan-in-scaled word 0x1c637997ef8ba9f2 at fan-in 64;
    // - bias (1, 5) replacement: word 0x65b52417be69ee0a;
    // - perturbation of 0.25 by 0.02 × N, with N the Box–Muller cosine of
    //   words 0x91a09948ada727b2 and 0xc659cf0c5953de83 (0.1648969520382955).
    let catalog = catalog();
    let mut quota = quota();
    let parent = uniform_parent(0.25, &catalog, &mut quota);
    let shape = catalog
        .topology(catalog.default_topology())
        .expect("default member")
        .layers()[1];
    let weight = shape.weight(5, 17).expect("in range");
    let bias = shape.bias(5).expect("in range");
    let at = attempt(7, 1234);

    let rates = MutationParameters {
        replacement_probability: 1.0,
        ..quiet()
    };
    let (genome, _) = child(&mutator(rates, &catalog), &parent, at, &mut quota);
    assert_eq!(f32s(&genome)[weight].to_bits(), 0xbe2c_8834);
    assert_eq!(f32s(&genome)[bias].to_bits(), 0xbd36_28c2);

    let rates = MutationParameters {
        weight_probability: 1.0,
        weight_standard_deviation: 0.02,
        ..quiet()
    };
    let (genome, _) = child(&mutator(rates, &catalog), &parent, at, &mut quota);
    assert_eq!(f32s(&genome)[weight].to_bits(), 0x3e81_b044);
}

/// FL-030: the compact-F16 profile's rounding (plan §10.3, "F16 with
/// declared rounding"). Every value is computed in F64 from the parent
/// widened exactly and rounded once to binary16, ties to even; the
/// full-path bits are `numpy.float16` of the F64 values the F32 fixtures
/// above round to F32 (NumPy 2.5.4, imports no FrankenLife code).
#[test]
fn canonical_f16_rounding_fixtures() {
    let one = F16::from_f64(1.0);
    let step = 2.0_f64.powi(-10);
    assert_eq!(perturb_weight(one, step / 4.0), one);
    assert_eq!(perturb_weight(one, step / 2.0), one, "a tie rounds to even");
    assert_eq!(perturb_weight(one, 0.75 * step).to_bits(), 0x3c01);
    assert_eq!(
        perturb_weight(F16::from_bits(0x3c01), step / 2.0).to_bits(),
        0x3c02,
        "a tie rounds to even"
    );
    assert_eq!(
        perturb_weight(F16::ZERO, 1.0e-7).to_bits(),
        0x0002,
        "subnormal"
    );

    let catalog = catalog();
    let mut quota = quota().with_dtype(WeightDtype::F16);
    let parent = uniform_parent(0.25, &catalog, &mut quota);
    assert_eq!(parent.parameters().dtype(), WeightDtype::F16);
    let shape = catalog
        .topology(catalog.default_topology())
        .expect("default member")
        .layers()[1];
    let weight = shape.weight(5, 17).expect("in range");
    let bias = shape.bias(5).expect("in range");
    let at = attempt(7, 1234);
    let halves = |genome: &Genome| {
        let values = genome.parameters().as_f16().expect("an F16 genome");
        (values[weight].to_bits(), values[bias].to_bits())
    };

    let rates = MutationParameters {
        replacement_probability: 1.0,
        ..quiet()
    };
    let (genome, _) = child(&mutator(rates, &catalog), &parent, at, &mut quota);
    assert_eq!(halves(&genome), (0xb164, 0xa9b1));

    let rates = MutationParameters {
        weight_probability: 1.0,
        weight_standard_deviation: 0.02,
        ..quiet()
    };
    let (genome, _) = child(&mutator(rates, &catalog), &parent, at, &mut quota);
    assert_eq!(halves(&genome).0, 0x340e);
}

#[test]
fn topology_changes_choose_uniformly_among_the_other_admitted_members() {
    let rates = MutationParameters {
        topology_change_probability: 1.0,
        ..quiet()
    };
    let attempts = 400_u64;
    for (cap, members) in [(30_000, 5), (20_000, 4)] {
        let config = appendix_a();
        let catalog = TopologyCatalog::dense_104_36_v1(cap, &[64, 32]).expect("catalog");
        assert_eq!(
            config.config().architecture_mutation.maximum_parameters,
            30_000
        );
        let mut quota = quota();
        let parent = founder_of(&SemanticRng::new(SEED), 5, &catalog, &mut quota);
        let mutator = mutator(rates, &catalog);
        let mut counts = BTreeMap::new();
        for tick in 0..attempts {
            let (genome, record) = child(&mutator, &parent, attempt(5, tick), &mut quota.clone());
            let change = record.topology.expect("probability one");
            assert_ne!(change.to, parent.topology());
            assert!(catalog.admits(change.to));
            *counts.entry(genome.topology().index()).or_insert(0_u64) += 1;
        }
        let others = members - 1;
        assert_eq!(counts.len(), others, "{counts:?}");
        let p = 1.0 / others as f64;
        let n = attempts as f64;
        let sigma = (n * p * (1.0 - p)).sqrt();
        for (&member, &count) in &counts {
            assert!(
                (count as f64 - n * p).abs() < 5.0 * sigma,
                "member {member}: {count} of {attempts}"
            );
        }
    }
}

#[test]
fn perturbation_and_replacement_counts_follow_their_rates() {
    // Appendix A's rates; counts over 200 children of one parent lie within
    // five standard deviations of their binomial means, and the changes
    // are 0.02 × N(0, 1) before rounding.
    let catalog = catalog();
    let rates = MutationParameters::from_config(&appendix_a()).expect("Appendix A rates");
    let rates = MutationParameters {
        topology_change_probability: 0.0,
        ..rates
    };
    let mutator = mutator(rates, &catalog);
    let mut quota = quota();
    let parent = founder_of(&SemanticRng::new(SEED), 6, &catalog, &mut quota);
    let size = parent.parameters().len() as f64;
    let children = 200_u64;
    let (mut perturbed, mut replaced) = (0_u64, 0_u64);
    let (mut sum, mut squares, mut changes) = (0.0, 0.0, 0.0);
    for tick in 0..children {
        let (genome, record) = child(&mutator, &parent, attempt(6, tick), &mut quota.clone());
        perturbed += u64::from(record.perturbed);
        replaced += u64::from(record.replaced);
        if record.replaced == 0 {
            for (&ours, &theirs) in f32s(&genome).iter().zip(f32s(&parent)) {
                if ours != theirs {
                    let step = (f64::from(ours) - f64::from(theirs)) / 0.02;
                    sum += step;
                    squares += step * step;
                    changes += 1.0;
                }
            }
        }
    }
    let n = children as f64 * size;
    for (count, p) in [(perturbed, 0.002 * (1.0 - 0.0001)), (replaced, 0.0001)] {
        let mean = n * p;
        let sigma = (n * p * (1.0 - p)).sqrt();
        assert!(
            (count as f64 - mean).abs() < 5.0 * sigma,
            "{count} against {mean} ± {sigma}"
        );
    }
    let mean = sum / changes;
    let variance = squares / changes - mean * mean;
    assert!(changes > 1_000.0);
    assert!(mean.abs() < 5.0 / changes.sqrt(), "{mean}");
    assert!(
        (variance - 1.0).abs() < 5.0 * (2.0 / changes).sqrt(),
        "{variance}"
    );
}

#[test]
fn sensing_scale_reflects_at_its_log_bounds() {
    let range = [1.0, 16.0];
    let reflect =
        |scale: f64, delta: f64| reflect_log_scale(scale, delta, range).expect("reflects");
    // Inside: `exp(ln s + δ)`.
    assert!((reflect(2.0, 0.1) - 2.0 * 0.1_f64.exp()).abs() < 1.0e-15);
    // Below the floor `ln 1 = 0`: `δ = −0.1` reflects to `+0.1`.
    assert_eq!(reflect(1.0, -0.1), exp(0.1).expect("finite"));
    // Above the ceiling: `ln 16 + 0.5` reflects to `ln 16 − 0.5`.
    assert!((reflect(16.0, 0.5) - 16.0 * (-0.5_f64).exp()).abs() < 1.0e-14);
    // Repeated reflection stays inside, and the bounds themselves hold.
    for step in [-20.0, -5.5, -0.01, 0.0, 0.01, 3.3, 11.0, 20.0] {
        for scale in [1.0, 1.5, 4.0, 16.0] {
            let value = reflect(scale, step);
            assert!((1.0..=16.0).contains(&value), "{scale} {step} -> {value}");
        }
    }
    // A single admitted scale admits only itself.
    assert_eq!(reflect_log_scale(3.0, 0.4, [3.0, 3.0]), Ok(3.0));
    // A step that cannot be reflected inside is refused, not clamped.
    for refused in [
        reflect_log_scale(2.0, 1.0e6, range),
        reflect_log_scale(2.0, f64::NAN, range),
        reflect_log_scale(0.0, 0.1, range),
    ] {
        assert!(matches!(refused, Err(CoreError::NumericalInvalid { .. })));
    }
}

#[test]
fn rates_come_from_the_configuration_and_are_validated() {
    let rates = MutationParameters::from_config(&appendix_a()).expect("Appendix A rates");
    assert_eq!(
        rates,
        MutationParameters {
            topology_change_probability: 0.01,
            weight_probability: 0.002,
            weight_standard_deviation: 0.02,
            replacement_probability: 0.0001,
            chemistry_standard_deviation: 0.01,
            sensing_log_scale_standard_deviation: 0.02,
            reject_effective_noop: true,
            maximum_candidates: 8,
        }
    );
    for invalid in [
        MutationParameters {
            maximum_candidates: 9,
            ..rates
        },
        MutationParameters {
            maximum_candidates: 0,
            ..rates
        },
        MutationParameters {
            weight_probability: 1.5,
            ..rates
        },
        MutationParameters {
            topology_change_probability: f64::NAN,
            ..rates
        },
        MutationParameters {
            weight_standard_deviation: -0.1,
            ..rates
        },
        MutationParameters {
            sensing_log_scale_standard_deviation: f64::INFINITY,
            ..rates
        },
        // A strict rule with no operator able to change a genome (§7.3).
        MutationParameters {
            reject_effective_noop: true,
            ..quiet()
        },
        // Perturbation with a zero deviation changes nothing either.
        MutationParameters {
            reject_effective_noop: true,
            weight_probability: 1.0,
            ..quiet()
        },
    ] {
        assert!(
            matches!(invalid.validate(), Err(CoreError::NumericalInvalid { .. })),
            "{invalid:?}"
        );
        assert!(Mutator::new(SemanticRng::new(SEED), invalid, &catalog(), limits()).is_err());
    }
    // Architecture mutation switched off zeroes the topology probability.
    let text = PLAN[PLAN.find("## Appendix A").unwrap_or(0)..]
        .split("```toml\n")
        .nth(1)
        .and_then(|block| block.split("```").next())
        .unwrap_or("")
        .replacen("enabled = true\ncatalog", "enabled = false\ncatalog", 1);
    let config = ValidatedConfig::from_toml(&text).expect("still valid");
    assert!(!config.config().architecture_mutation.enabled);
    let rates = MutationParameters::from_config(&config).expect("rates");
    assert_eq!(rates.topology_change_probability, 0.0);
}

/// Empty values in `dtype`.
fn empty_values(dtype: WeightDtype) -> ParameterValues {
    match dtype {
        WeightDtype::F32 => ParameterValues::F32(Vec::new()),
        WeightDtype::F16 => ParameterValues::F16(Vec::new()),
    }
}

/// Candidate 0 of `at` before mutation: its member and values.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn unmutated(
    mutator: &Mutator<'_>,
    parent: &Genome,
    at: BirthAttempt,
) -> (TopologyId, ParameterValues) {
    let mut values = empty_values(parent.parameters().dtype());
    let member = mutator
        .unmutated(parent, at, &mut values)
        .expect("candidate 0 maps");
    (member, values)
}

/// The reference child's candidate 0, as `propose_prepared` takes it.
fn prepared_from<'g>(genome: &'g Genome, record: &flife_genome::MutationRecord) -> Prepared<'g> {
    Prepared {
        topology: genome.topology(),
        values: genome.parameters(),
        replaced: record.replaced,
        perturbed: record.perturbed,
    }
}

#[test]
fn unmutated_candidates_are_the_reference_children_before_step_2() {
    // With every parameter operator off, a child is its candidate 0 before
    // step 2: the parent copied, or mapped onto another member.
    let catalog = catalog();
    let rates = MutationParameters {
        topology_change_probability: 0.5,
        ..quiet()
    };
    let mutator = mutator(rates, &catalog);
    for dtype in [WeightDtype::F32, WeightDtype::F16] {
        let mut quota = quota().with_dtype(dtype);
        let (mut mapped, mut copied) = (0, 0);
        for organism in 1..=4 {
            let parent = founder_of(&SemanticRng::new(SEED), organism, &catalog, &mut quota);
            for tick in 0..40 {
                let at = attempt(organism, tick);
                let (genome, record) = child(&mutator, &parent, at, &mut quota.clone());
                assert_eq!(record.candidate, 0);
                let (member, values) = unmutated(&mutator, &parent, at);
                assert_eq!(member, genome.topology(), "{dtype:?} {organism} {tick}");
                assert!(values.weights().same_bits(&genome.parameters()));
                if member == parent.topology() {
                    copied += 1;
                } else {
                    mapped += 1;
                }
                // A second candidate is appended after the first.
                let mut both = values.clone();
                mutator
                    .unmutated(&parent, at, &mut both)
                    .expect("candidate 0 maps again");
                let length = values.weights().len();
                assert_eq!(both.weights().len(), 2 * length);
                let tail: Vec<u32> = both.weights().bits().skip(length).collect();
                assert!(tail.iter().copied().eq(values.weights().bits()));
            }
        }
        assert!(
            mapped > 0 && copied > 0,
            "{dtype:?}: {mapped} mapped, {copied} copied"
        );
    }
}

#[test]
fn a_prepared_first_candidate_is_proposed_as_the_reference_proposes_it() {
    // Candidate 0 as the reference mutated it: its traits, seal and record
    // are the reference's, and so is the quota after it.
    let catalog = catalog();
    let rates = MutationParameters {
        topology_change_probability: 0.3,
        weight_probability: 0.05,
        weight_standard_deviation: 0.5,
        replacement_probability: 0.01,
        reject_effective_noop: true,
        ..MutationParameters::from_config(&appendix_a()).expect("Appendix A rates")
    };
    let mutator = mutator(rates, &catalog);
    for dtype in [WeightDtype::F32, WeightDtype::F16] {
        let mut quota = quota().with_dtype(dtype);
        let (mut mapped, mut compared) = (0, 0);
        for organism in 1..=4 {
            let parent = founder_of(&SemanticRng::new(SEED), organism, &catalog, &mut quota);
            for tick in 0..25 {
                let at = attempt(organism, tick);
                let mut theirs = quota.clone();
                let (genome, record) = child(&mutator, &parent, at, &mut theirs);
                if record.candidate != 0 {
                    continue;
                }
                let mut ours = quota.clone();
                let proposal = mutator
                    .propose_prepared(
                        &parent,
                        at,
                        GenomeId::new(5_000),
                        &mut ours,
                        prepared_from(&genome, &record),
                    )
                    .expect("no capacity error");
                let Proposal::Child {
                    genome: prepared,
                    record: prepared_record,
                } = proposal
                else {
                    panic!("{dtype:?} {organism} {tick}: the prepared birth was rejected");
                };
                assert_eq!(content(&prepared), content(&genome));
                assert_eq!(prepared_record, record);
                assert_eq!(ours, theirs);
                compared += 1;
                mapped += usize::from(record.topology.is_some());
            }
        }
        assert!(
            compared > 90 && mapped > 0,
            "{dtype:?}: {compared} compared, {mapped} mapped"
        );
    }
}

#[test]
fn a_prepared_candidates_values_and_counts_are_taken_as_given() {
    let catalog = catalog();
    let rates = MutationParameters {
        topology_change_probability: 0.0,
        ..MutationParameters::from_config(&appendix_a()).expect("Appendix A rates")
    };
    let mutator = mutator(rates, &catalog);
    let mut quota = quota();
    let parent = founder_of(&SemanticRng::new(SEED), 7, &catalog, &mut quota);
    let at = attempt(7, 3);
    let (genome, _) = child(&mutator, &parent, at, &mut quota.clone());
    let mut altered = f32s(&genome).to_vec();
    altered[17] = f32::from_bits(altered[17].to_bits() ^ 1);
    let proposal = mutator
        .propose_prepared(
            &parent,
            at,
            GenomeId::new(5_000),
            &mut quota.clone(),
            Prepared {
                topology: genome.topology(),
                values: Weights::F32(&altered),
                replaced: 7,
                perturbed: 9,
            },
        )
        .expect("no capacity error");
    let Proposal::Child {
        genome: prepared,
        record: prepared_record,
    } = proposal
    else {
        panic!("the prepared birth was rejected");
    };
    assert_eq!(f32s(&prepared), altered.as_slice());
    assert_eq!(content(&prepared).2, content(&genome).2, "the traits");
    assert_eq!(
        (prepared_record.replaced, prepared_record.perturbed),
        (7, 9)
    );
}

#[test]
fn a_prepared_candidate_of_another_member_or_layout_is_refused_without_charge() {
    let catalog = catalog();
    // Only the weights mutate, so an unmutated candidate is its parent.
    let rates = MutationParameters {
        weight_probability: 0.002,
        weight_standard_deviation: 0.02,
        reject_effective_noop: true,
        ..quiet()
    };
    let mutator = mutator(rates, &catalog);
    let mut half_quota = quota().with_dtype(WeightDtype::F16);
    let mut quota = quota();
    let parent = founder_of(&SemanticRng::new(SEED), 8, &catalog, &mut quota);
    let at = attempt(8, 5);
    let (member, values) = unmutated(&mutator, &parent, at);
    assert_eq!(member, parent.topology());
    let other = catalog
        .transition_targets(member)
        .next()
        .expect("another member");
    let full = values.weights().as_f32().expect("F32 values");
    let half: Vec<F16> = full.iter().map(|&v| F16::from_f64(f64::from(v))).collect();
    let half_parent = founder_of(&SemanticRng::new(SEED), 8, &catalog, &mut half_quota);
    assert_eq!(half_parent.topology(), member);
    let prepared = |topology, values| Prepared {
        topology,
        values,
        replaced: 0,
        perturbed: 0,
    };
    // Another member; then, for each dtype's parent, values one short and
    // values of the other dtype.
    let wide = (&parent, &quota);
    let narrow = (&half_parent, &half_quota);
    for ((owner, held), first, conflict) in [
        (wide, prepared(other, Weights::F32(full)), true),
        (wide, prepared(member, Weights::F32(&full[1..])), false),
        (wide, prepared(member, Weights::F16(&half)), false),
        (narrow, prepared(member, Weights::F16(&half[1..])), false),
        (narrow, prepared(member, Weights::F32(full)), false),
    ] {
        let mut charged = held.clone();
        let result = mutator.propose_prepared(owner, at, GenomeId::new(5_000), &mut charged, first);
        if conflict {
            assert!(
                matches!(result, Err(CoreError::StateConflict { .. })),
                "{result:?}"
            );
        } else {
            assert!(
                matches!(result, Err(CoreError::LayoutMismatch { .. })),
                "{result:?}"
            );
        }
        assert_eq!(&charged, held, "nothing charged");
    }
    // Values of another dtype are refused, and left as they were.
    let mut wrong = ParameterValues::F16(vec![F16::ZERO; 3]);
    assert!(matches!(
        mutator.unmutated(&parent, at, &mut wrong),
        Err(CoreError::LayoutMismatch { .. })
    ));
    assert_eq!(wrong, ParameterValues::F16(vec![F16::ZERO; 3]));
    // An unmutated candidate 0 is its parent: the no-op check rejects it,
    // and the next candidate is the mutator's own.
    let proposal = mutator
        .propose_prepared(
            &parent,
            at,
            GenomeId::new(5_000),
            &mut quota.clone(),
            Prepared {
                topology: member,
                values: values.weights(),
                replaced: 0,
                perturbed: 0,
            },
        )
        .expect("no capacity error");
    let Proposal::Child { record, .. } = proposal else {
        panic!("every candidate was rejected");
    };
    assert_eq!(record.candidate, 1);
    assert_eq!(
        record.rejected,
        vec![flife_genome::CandidateRejection {
            candidate: 0,
            reason: RejectionReason::EffectiveNoop,
        }]
    );
}
