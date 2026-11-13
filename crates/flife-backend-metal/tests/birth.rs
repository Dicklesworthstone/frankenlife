//! FL-032: phase 8's parameter mutation on Metal is flife-genome's, bit for
//! bit. The device's `exp`, `ln`, `sin_cos_turns`, `open_unit`, Box–Muller,
//! binary16 conversions and fan-in draw are checked against flife-core's and
//! flife-genome's on random and edge arguments, every binary16 pattern
//! included. The kernel is checked against `Mutator::propose` on two
//! generations of children, in F32 and F16, under seven rate sets: the
//! preset's, every weight perturbed, every weight replaced, a mix with
//! topology changes (the children mapped onto other members first, and many
//! first candidates rejected), a replacement and a weight probability
//! exactly at a parameter's own draw, and perturbations below half a
//! binary16 step. A rate the reference refuses is refused.

use flife_backend_metal::{
    MUTATION_LAYERS, MetalBirth, MutationBirth, MutationConstants, MutationCounts, MutationLayer,
    ParameterBits,
};
use flife_core::binary16::F16;
use flife_core::ids::{GenomeId, OrganismId};
use flife_core::numerics::{exp, ln, sin_cos_turns};
use flife_core::rng::{
    Address, Lane, LaneKind, SemanticRng, Stream, box_muller, half_open_unit, open_unit,
};
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_genome::{
    BirthAttempt, Genome, GenomeQuota, MutationParameters, Mutator, Proposal, TopologyCatalog,
    TraitLimits, WeightDtype, fan_in_draw, founder, initial_scale, map_parameters,
};
use flife_neural::DenseTopology;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");
const SEED: u64 = 42;

/// `None` off macOS, where `open` must refuse with its typed error.
#[expect(clippy::panic, reason = "test helper")]
fn metal() -> Option<MetalBirth> {
    match MetalBirth::open() {
        Ok(birth) => Some(birth),
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => None,
        Err(error) => panic!("Metal birth kernels did not open: {error}"),
    }
}

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn unit(&mut self) -> f64 {
        half_open_unit(self.next())
    }
}

fn pair(row: &[u32], at: usize) -> u64 {
    u64::from(row[at]) | (u64::from(row[at + 1]) << 32)
}

/// One argument row for `birth_ops`: `[x, y, u, w1, w2, f, h, fan_in]`.
fn arguments(rng: &mut Rng, index: usize) -> [u64; 8] {
    let x = match rng.below(8) {
        0 => f64::from_bits(rng.next()),
        1 => (rng.unit() - 0.5) * 2.0 * 0.0625,
        2 => 709.0 + rng.unit(),
        3 => -740.0 - 10.0 * rng.unit(),
        _ => (rng.unit() - 0.5) * 1_500.0,
    };
    let x = if x.is_finite() { x } else { rng.unit() };
    let y = match rng.below(6) {
        0 => f64::from_bits(rng.below(0x7FF0_0000_0000_0000)),
        1 => open_unit(rng.next()),
        2 => 1.0 + (rng.unit() - 0.5) * 1e-6,
        3 => std::f64::consts::SQRT_2 * (1.0 + (rng.unit() - 0.5) * 1e-12),
        _ => f64::from_bits(rng.below(1 << 52)),
    };
    let y = if y > 0.0 && y.is_finite() { y } else { 1.0 };
    let u = match rng.below(4) {
        0 => f64::from(u32::try_from(rng.below(8)).unwrap_or(0)) / 8.0,
        1 => (f64::from(u32::try_from(rng.below(4)).unwrap_or(0)) / 4.0 + 0.125).next_up(),
        _ => rng.unit(),
    };
    let u = if (0.0..1.0).contains(&u) { u } else { 0.0 };
    let f = match rng.below(6) {
        0 => f64::from_bits(rng.next()),
        // Binary16's normal and subnormal ranges, and its ties.
        1 => (rng.unit() - 0.5) * 140_000.0,
        2 => (rng.unit() - 0.5) * 1e-4,
        3 => {
            F16::from_bits(u16::try_from(rng.below(1 << 16)).unwrap_or(0)).to_f64()
                + (f64::from(u32::try_from(rng.below(3)).unwrap_or(0)) - 1.0) * 2.0_f64.powi(-25)
        }
        _ => (rng.unit() - 0.5) * 2.0,
    };
    let fan_in = match rng.below(4) {
        0 => 1 + rng.below(4),
        1 => [32, 36, 64, 104][usize::try_from(rng.below(4)).unwrap_or(0)],
        _ => 1 + rng.below(1 << 20),
    };
    [
        x.to_bits(),
        y.to_bits(),
        u.to_bits(),
        rng.next(),
        rng.next(),
        f.to_bits(),
        // Every binary16 pattern once.
        u64::try_from(index % (1 << 16)).unwrap_or(0),
        fan_in,
    ]
}

/// Argument rows at the edges.
fn edges() -> Vec<[u64; 8]> {
    let xs = [
        0.0,
        -0.0,
        0.0625,
        0.0625_f64.next_up(),
        -0.0625,
        -0.0625_f64.next_down(),
        709.78,
        709.782_712_893_384,
        710.0_f64.next_down(),
        710.0,
        -745.13,
        -746.0,
        -746.0_f64.next_up(),
        f64::from_bits(1),
        -f64::from_bits(1),
        std::f64::consts::LN_2 * 0.5,
        std::f64::consts::LN_2 * 1.5,
        1.0,
        -1.0,
    ];
    let ys = [
        1.0,
        1.0_f64.next_up(),
        1.0_f64.next_down(),
        std::f64::consts::SQRT_2,
        std::f64::consts::SQRT_2.next_up(),
        std::f64::consts::SQRT_2.next_down(),
        f64::MIN_POSITIVE,
        f64::MIN_POSITIVE.next_down(),
        f64::from_bits(1),
        f64::MAX,
        0.5,
        2.0,
        open_unit(0),
        open_unit(u64::MAX),
    ];
    let us = [
        0.0,
        -0.0,
        0.125,
        0.25,
        0.375,
        0.5,
        0.625,
        0.75,
        0.875,
        0.25_f64.next_down(),
        0.25_f64.next_up(),
        0.125_f64.next_up(),
        0.125_f64.next_down(),
        1.0_f64.next_down(),
    ];
    let words = [0, u64::MAX, (1 << 12) - 1, 1 << 12, 1 << 63, u64::MAX >> 12];
    let fs = [
        0.0,
        -0.0,
        65_504.0,
        65_519.0,
        65_520.0,
        -65_520.0,
        2.0_f64.powi(-24),
        2.0_f64.powi(-25),
        2.0_f64.powi(-25).next_up(),
        3.0 * 2.0_f64.powi(-25),
        2.0_f64.powi(-14),
        2.0_f64.powi(-14).next_down(),
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        f64::from_bits(1),
        1.0 + 2.0_f64.powi(-11),
        1.0 + 3.0 * 2.0_f64.powi(-11),
    ];
    let fan_ins = [1, 2, 3, 104, u64::from(u32::MAX)];
    let count = [
        xs.len(),
        ys.len(),
        us.len(),
        words.len(),
        fs.len(),
        fan_ins.len(),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    (0..count)
        .map(|k| {
            [
                xs[k % xs.len()].to_bits(),
                ys[k % ys.len()].to_bits(),
                us[k % us.len()].to_bits(),
                words[k % words.len()],
                words[(k + 1) % words.len()],
                fs[k % fs.len()].to_bits(),
                0x7C00 + u64::try_from(k).unwrap_or(0),
                fan_ins[k % fan_ins.len()],
            ]
        })
        .collect()
}

#[test]
fn the_arithmetic_is_flife_cores() {
    let Some(birth) = metal() else { return };
    let mut rng = Rng(0x0BAD_5EED_1234_5678);
    let mut rows = edges();
    let total = (1 << 16) + 4_096;
    let start = rows.len();
    rows.extend((start..total).map(|index| arguments(&mut rng, index)));
    while !rows.len().is_multiple_of(64) {
        let index = rows.len();
        rows.push(arguments(&mut rng, index));
    }
    let mut out = Vec::new();
    birth
        .birth_ops(&rows, &mut out)
        .expect("the device computes");
    assert_eq!(out.len(), rows.len());
    let (mut undefined, mut subnormal_logs, mut fan_ins) = (0, 0, 0);
    for (row, got) in rows.iter().zip(&out) {
        let [x, y, u, first, second, f, h, fan_in] = *row;
        let (x, y, u, f) = (
            f64::from_bits(x),
            f64::from_bits(y),
            f64::from_bits(u),
            f64::from_bits(f),
        );
        match exp(x) {
            Some(value) => {
                assert_eq!(pair(got, 0), 1, "exp({x:e}) is defined");
                assert_eq!(pair(got, 2), value.to_bits(), "exp({x:e})");
            }
            None => {
                assert_eq!(pair(got, 0), 0, "exp({x:e}) overflows");
                undefined += 1;
            }
        }
        let natural = ln(y).expect("a positive finite argument");
        assert_eq!(pair(got, 4), natural.to_bits(), "ln({y:e})");
        subnormal_logs += usize::from(y < f64::MIN_POSITIVE);
        let (sin, cos) = sin_cos_turns(u).expect("a turn in [0, 1)");
        assert_eq!(pair(got, 6), sin.to_bits(), "sin of turn {u:e}");
        assert_eq!(pair(got, 8), cos.to_bits(), "cos of turn {u:e}");
        assert_eq!(
            pair(got, 10),
            open_unit(first).to_bits(),
            "open_unit({first:#x})"
        );
        let normals = box_muller(first, second).expect("words always give normals");
        assert_eq!(
            pair(got, 12),
            normals[0].to_bits(),
            "Box–Muller 0 of {first:#x}"
        );
        assert_eq!(
            pair(got, 14),
            normals[1].to_bits(),
            "Box–Muller 1 of {first:#x}"
        );
        assert_eq!(
            pair(got, 16),
            u64::from(F16::from_f64(f).to_bits()),
            "binary16 of {f:e}"
        );
        let half = F16::from_bits(u16::try_from(h).expect("binary16 bits"));
        assert_eq!(
            pair(got, 18),
            u64::from(half.to_f32().to_bits()),
            "binary32 of binary16 {h:#x}"
        );
        let fan_in = usize::try_from(fan_in).expect("a fan-in");
        let drawn = (2.0 * half_open_unit(first) - 1.0) * (3.0 / fan_in as f64).sqrt();
        assert_eq!(pair(got, 20), drawn.to_bits(), "fan-in draw of {fan_in}");
        let single: f32 = fan_in_draw(first, fan_in).expect("a fan-in draw");
        let compact: F16 = fan_in_draw(first, fan_in).expect("a fan-in draw");
        assert_eq!(got[22], single.to_bits(), "F32 fan-in draw of {fan_in}");
        assert_eq!(
            got[23],
            u32::from(compact.to_bits()),
            "F16 fan-in draw of {fan_in}"
        );
        fan_ins += usize::from(fan_in > 1);
    }
    println!(
        "{} rows: exp undefined {undefined}, subnormal ln {subnormal_logs}",
        rows.len()
    );
    assert!(undefined > 0 && subnormal_logs > 0 && fan_ins > 0);
}

/// A child the reference proposed, and what the device starts from: the
/// parent's parameters, or the parent's mapped onto the child's member.
struct Child<W> {
    parent: u64,
    tick: u64,
    candidate: u8,
    topology: flife_genome::TopologyId,
    mapped: bool,
    start: Vec<W>,
    expected: Vec<W>,
    counts: MutationCounts,
    /// The child, until it becomes a parent.
    genome: Option<Genome>,
}

/// The half-open uniform of word `word` of draw 0 for founder 1's first
/// weight at tick 0, candidate 0.
#[expect(clippy::expect_used, reason = "test helper")]
fn first_weight_uniform(word: usize) -> f64 {
    let index = Lane::neuron_index(0, 0, 0).expect("a neuron's lane");
    let lane = Lane::new(0, LaneKind::Weight, index).expect("a weight lane");
    half_open_unit(
        SemanticRng::new(SEED).block(Address {
            stream: Stream::Mutation,
            entity: 1,
            ordinal: 0,
            lane,
            draw: 0,
        })[word],
    )
}

/// An F32 genome's parameters.
#[expect(clippy::expect_used, reason = "test helper")]
fn f32s(genome: &Genome) -> &[f32] {
    genome.parameters().as_f32().expect("an F32 genome")
}

/// An F16 genome's parameters.
#[expect(clippy::expect_used, reason = "test helper")]
fn f16s(genome: &Genome) -> &[F16] {
    genome.parameters().as_f16().expect("an F16 genome")
}

/// The children's genomes as the next generation's parents, numbered from
/// `first`.
fn next_generation<W>(children: &mut [Child<W>], first: u64) -> Vec<(u64, Genome)> {
    (first..)
        .zip(children.iter_mut().filter_map(|child| child.genome.take()))
        .collect()
}

#[expect(clippy::expect_used, reason = "test helper")]
#[expect(
    clippy::too_many_arguments,
    reason = "the mutator, its catalog, the parents, the ticks, the quota and the dtype"
)]
fn children<W: ParameterBits + Copy>(
    mutator: &Mutator<'_>,
    catalog: &TopologyCatalog,
    parents: &[(u64, Genome)],
    ticks: &[u64],
    quota: &mut GenomeQuota,
    next_genome: &mut u64,
    values: impl Fn(&Genome) -> &[W],
    zero: W,
) -> Vec<Child<W>> {
    let rng = SemanticRng::new(SEED);
    let mut children = Vec::new();
    for (organism, parent) in parents {
        for &tick in ticks {
            *next_genome += 1;
            let proposal = mutator
                .propose(
                    parent,
                    BirthAttempt {
                        parent: OrganismId::new(*organism),
                        tick: Tick::new(tick),
                    },
                    GenomeId::new(*next_genome),
                    quota,
                )
                .expect("a proposal");
            let Proposal::Child { genome, record } = proposal else {
                continue;
            };
            let mapped = record.topology.is_some();
            let start = match record.topology {
                None => values(parent).to_vec(),
                Some(change) => {
                    let source = catalog.topology(change.from).expect("the parent's member");
                    let target = catalog.topology(change.to).expect("the child's member");
                    let mut start = vec![zero; target.parameter_count()];
                    map_parameters(source, values(parent), target, &mut start, |at| {
                        let index = Lane::neuron_index(at.layer, at.output, at.input)
                            .expect("a neuron's lane");
                        let lane = Lane::new(record.candidate, LaneKind::MapWeight, index)
                            .expect("a map lane");
                        rng.block(Address {
                            stream: Stream::Mutation,
                            entity: *organism,
                            ordinal: tick,
                            lane,
                            draw: 0,
                        })[0]
                    })
                    .expect("the reference's map");
                    start
                }
            };
            children.push(Child {
                parent: *organism,
                tick,
                candidate: record.candidate,
                topology: genome.topology(),
                mapped,
                start,
                expected: values(&genome).to_vec(),
                counts: MutationCounts {
                    replaced: record.replaced,
                    perturbed: record.perturbed,
                },
                genome: Some(*genome),
            });
        }
    }
    children
}

/// Every child mutated on the device in one call: each value and count the
/// reference's. Returns the counts seen: (children, mapped, replaced,
/// perturbed, later candidates).
#[expect(clippy::expect_used, reason = "test helper")]
fn race<W: ParameterBits + Copy>(
    birth: &mut MetalBirth,
    rates: &MutationParameters,
    catalog: &TopologyCatalog,
    children: &[Child<W>],
    name: &str,
) -> [usize; 5] {
    let births: Vec<MutationBirth> = children
        .iter()
        .map(|child| {
            birth_of(
                child.parent,
                child.tick,
                child.candidate,
                catalog.topology(child.topology).expect("a member"),
            )
        })
        .collect();
    let mut values: Vec<W> = children
        .iter()
        .flat_map(|child| child.start.clone())
        .collect();
    let mut counts = Vec::new();
    birth
        .mutate(
            &MutationConstants {
                seed: SEED,
                replacement_probability: rates.replacement_probability,
                weight_probability: rates.weight_probability,
                weight_standard_deviation: rates.weight_standard_deviation,
            },
            &births,
            &mut values,
            &mut counts,
        )
        .expect("the device mutates");
    assert_eq!(counts.len(), children.len());
    let mut seen = [children.len(), 0, 0, 0, 0];
    let mut at = 0;
    for (child, got) in children.iter().zip(&counts) {
        let end = at + child.expected.len();
        for (k, (device, reference)) in values[at..end].iter().zip(&child.expected).enumerate() {
            assert_eq!(
                device.bits(),
                reference.bits(),
                "{name}: parameter {k} of the child of {} at tick {}",
                child.parent,
                child.tick
            );
        }
        assert_eq!(
            *got, child.counts,
            "{name}: counts of the child of {}",
            child.parent
        );
        seen[1] += usize::from(child.mapped);
        seen[2] += usize::try_from(got.replaced).unwrap_or(0);
        seen[3] += usize::try_from(got.perturbed).unwrap_or(0);
        seen[4] += usize::from(child.candidate > 0);
        at = end;
    }
    seen
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "two dtypes, five rate sets, two generations"
)]
fn mutations_on_the_device_are_the_references() {
    let Some(mut birth) = metal() else { return };
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let catalog = TopologyCatalog::from_config(&config).expect("the preset's catalog");
    let limits = TraitLimits::from_config(&config).expect("the preset's limits");
    let preset = MutationParameters::from_config(&config).expect("the preset's rates");
    let quiet = MutationParameters {
        topology_change_probability: 0.0,
        weight_probability: 0.0,
        weight_standard_deviation: 0.0,
        replacement_probability: 0.0,
        ..preset
    };
    let rate_sets = [
        ("the preset's", preset),
        (
            "every weight perturbed",
            MutationParameters {
                weight_probability: 1.0,
                weight_standard_deviation: 0.02,
                ..quiet
            },
        ),
        (
            "every weight replaced",
            MutationParameters {
                replacement_probability: 1.0,
                ..quiet
            },
        ),
        (
            // About one weight in a child past the bound of 4, so that many
            // first candidates are rejected and later ones accepted.
            "a mix with topology changes",
            MutationParameters {
                replacement_probability: 0.05,
                weight_probability: 0.01,
                weight_standard_deviation: 1.5,
                topology_change_probability: 0.5,
                ..quiet
            },
        ),
        // Founder 1's first weight at tick 0 exactly at its own draws: the
        // reference neither replaces nor perturbs it (`u < p`).
        (
            "a replacement probability at a draw",
            MutationParameters {
                replacement_probability: first_weight_uniform(0),
                reject_effective_noop: false,
                ..quiet
            },
        ),
        (
            "a weight probability at a draw",
            MutationParameters {
                weight_probability: first_weight_uniform(2),
                weight_standard_deviation: 0.02,
                reject_effective_noop: false,
                ..quiet
            },
        ),
        (
            "below half a binary16 step",
            MutationParameters {
                weight_probability: 1.0,
                weight_standard_deviation: 1e-6,
                reject_effective_noop: false,
                ..quiet
            },
        ),
    ];
    let ticks = [0, 7, 1_000, u64::MAX - 1];
    let mut totals = [0_usize; 5];
    for dtype in [WeightDtype::F32, WeightDtype::F16] {
        for (name, rates) in rate_sets {
            let mutator =
                Mutator::new(SemanticRng::new(SEED), rates, &catalog, limits).expect("valid rates");
            let mut quota = GenomeQuota::new(1 << 34, 1 << 34)
                .expect("quotas in order")
                .with_dtype(dtype);
            let founders: Vec<(u64, Genome)> = (1..=6)
                .map(|organism| {
                    let genome = founder(
                        &SemanticRng::new(SEED),
                        OrganismId::new(organism),
                        GenomeId::new(organism),
                        &catalog,
                        &mut quota,
                        &limits,
                        initial_scale(&config),
                    )
                    .expect("a founder");
                    (organism, genome)
                })
                .collect();
            let mut next = 100;
            let seen: [usize; 5] = match dtype {
                WeightDtype::F32 => {
                    let values = f32s;
                    let mut first = children(
                        &mutator, &catalog, &founders, &ticks, &mut quota, &mut next, values,
                        0.0_f32,
                    );
                    let parents = next_generation(&mut first, 1_000);
                    let second = children(
                        &mutator,
                        &catalog,
                        &parents,
                        &ticks[..2],
                        &mut quota,
                        &mut next,
                        values,
                        0.0_f32,
                    );
                    let a = race(&mut birth, &rates, &catalog, &first, name);
                    let b = race(&mut birth, &rates, &catalog, &second, name);
                    std::array::from_fn(|k| a[k] + b[k])
                }
                WeightDtype::F16 => {
                    let values = f16s;
                    let zero = F16::from_bits(0);
                    let mut first = children(
                        &mutator, &catalog, &founders, &ticks, &mut quota, &mut next, values, zero,
                    );
                    let parents = next_generation(&mut first, 1_000);
                    let second = children(
                        &mutator,
                        &catalog,
                        &parents,
                        &ticks[..2],
                        &mut quota,
                        &mut next,
                        values,
                        zero,
                    );
                    let a = race(&mut birth, &rates, &catalog, &first, name);
                    let b = race(&mut birth, &rates, &catalog, &second, name);
                    std::array::from_fn(|k| a[k] + b[k])
                }
            };
            println!(
                "{dtype:?}, {name}: children {}, mapped {}, replaced {}, perturbed {}, later candidates {}",
                seen[0], seen[1], seen[2], seen[3], seen[4]
            );
            assert!(seen[0] > 0, "{dtype:?}, {name}: no child");
            for k in 0..5 {
                totals[k] += seen[k];
            }
        }
    }
    assert!(totals.iter().all(|&count| count > 0), "{totals:?}");
}

#[test]
fn rates_the_reference_refuses_are_refused() {
    let Some(mut birth) = metal() else { return };
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let catalog = TopologyCatalog::from_config(&config).expect("the preset's catalog");
    let topology = catalog
        .topology(catalog.default_topology())
        .expect("the default member");
    let valid = MutationConstants {
        seed: SEED,
        replacement_probability: 0.5,
        weight_probability: 0.5,
        weight_standard_deviation: 0.1,
    };
    let births = [birth_of(1, 2, 0, topology)];
    let original = vec![0.25_f32; topology.parameter_count()];
    for (constants, quantity) in [
        (
            MutationConstants {
                replacement_probability: 1.0_f64.next_up(),
                ..valid
            },
            "mutation.replacement_probability",
        ),
        (
            MutationConstants {
                replacement_probability: f64::NAN,
                ..valid
            },
            "mutation.replacement_probability",
        ),
        (
            MutationConstants {
                weight_probability: -f64::from_bits(1),
                ..valid
            },
            "mutation.weight_probability",
        ),
        (
            MutationConstants {
                weight_standard_deviation: -1.0,
                ..valid
            },
            "mutation.weight_standard_deviation",
        ),
        (
            MutationConstants {
                weight_standard_deviation: f64::INFINITY,
                ..valid
            },
            "mutation.weight_standard_deviation",
        ),
        (
            MutationConstants {
                weight_standard_deviation: f64::NAN,
                ..valid
            },
            "mutation.weight_standard_deviation",
        ),
    ] {
        let mut values = original.clone();
        let mut counts = vec![MutationCounts::default()];
        let result = birth.mutate(&constants, &births, &mut values, &mut counts);
        assert!(
            matches!(result, Err(CoreError::NumericalInvalid { quantity: q, .. }) if q == quantity),
            "{constants:?}: {result:?}"
        );
        assert!(counts.is_empty());
        assert_eq!(values, original);
    }
    // Values that are not the members' parameters, one short or one over.
    let mut counts = Vec::new();
    for length in [
        topology.parameter_count() - 1,
        topology.parameter_count() + 1,
    ] {
        let mut wrong = vec![0.25_f32; length];
        let result = birth.mutate(&valid, &births, &mut wrong, &mut counts);
        assert!(
            matches!(result, Err(CoreError::LayoutMismatch { .. })),
            "{length}: {result:?}"
        );
        assert!(wrong.iter().all(|&value| value == 0.25) && counts.is_empty());
    }
    // Births whose layers are not a dense member's: no layers (and so no
    // parameters, which the layers' end alone would admit), more than fit,
    // a layer's weights one past the last layer's end (over its own biases),
    // biases inside the weights, and a count past the layers' end.
    let fine = births[0];
    let mut gap = fine;
    gap.layers[1].weights += 1;
    let mut inside = fine;
    inside.layers[0].biases -= 1;
    let mut longer = fine;
    longer.parameters += 1;
    for misshapen in [
        MutationBirth {
            depth: 0,
            parameters: 0,
            ..fine
        },
        MutationBirth {
            depth: MUTATION_LAYERS + 1,
            ..fine
        },
        gap,
        inside,
        longer,
    ] {
        let mut values = vec![0.25_f32; misshapen.parameters];
        let result = birth.mutate(&valid, &[misshapen], &mut values, &mut counts);
        assert!(
            matches!(result, Err(CoreError::LayoutMismatch { .. })),
            "{misshapen:?}: {result:?}"
        );
        assert!(values.iter().all(|&value| value == 0.25) && counts.is_empty());
    }
    // Nothing to mutate.
    birth
        .mutate(&valid, &[], &mut Vec::<f32>::new(), &mut counts)
        .expect("an empty call");
    assert!(counts.is_empty());
}

/// A birth of `topology`'s member, as the port names it.
fn birth_of(parent: u64, tick: u64, candidate: u8, topology: &DenseTopology) -> MutationBirth {
    let mut layers = [MutationLayer::default(); MUTATION_LAYERS];
    for (slot, shape) in layers.iter_mut().zip(topology.layers()) {
        *slot = MutationLayer {
            inputs: shape.inputs,
            outputs: shape.outputs,
            weights: shape.weights,
            biases: shape.biases,
        };
    }
    MutationBirth {
        parent,
        tick,
        candidate,
        layers,
        depth: topology.layers().len(),
        parameters: topology.parameter_count(),
    }
}
