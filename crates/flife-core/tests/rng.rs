//! `fnp-philox-semantic-v1` (plan §16.2, A.3 item 3, INV-11): blocks
//! against NumPy 2.5.4's `Philox`, frozen stream codes and lane packing,
//! order and worker independence, and the draw conventions against
//! correctly rounded values.
//!
//! Every expected word below comes from NumPy itself, and every expected
//! float from Python `Decimal` at 80 digits, through an oracle script that
//! imports neither FrankenLife nor FrankenNumPy.

use std::collections::{BTreeMap, HashSet};

use flife_core::rng::{
    Address, Block, FNP_PHILOX_SEMANTIC_V1, Lane, LaneKind, SemanticRng, Stream, bernoulli,
    box_muller, choose, half_open_unit, open_unit,
};
use flife_core::{CoreError, ValidatedConfig};
use fnp_random::PhiloxRng;

const MAX: u64 = u64::MAX;

/// A total order on finite `f64` in which adjacent values differ by one.
fn ordinal(value: f64) -> i64 {
    let bits = value.to_bits();
    let magnitude = i64::try_from(bits & 0x7fff_ffff_ffff_ffff).unwrap_or(i64::MAX);
    if bits >> 63 == 0 {
        magnitude
    } else {
        -magnitude
    }
}

fn ulps(a: f64, b: f64) -> u64 {
    ordinal(a).abs_diff(ordinal(b))
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn lane(candidate: u8, kind: LaneKind, index: u64) -> Lane {
    Lane::new(candidate, kind, index).expect("lane in range")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn neuron(layer: usize, output: usize, input: usize) -> u64 {
    Lane::neuron_index(layer, output, input).expect("neuron in range")
}

fn address(stream: Stream, entity: u64, ordinal: u64, lane: Lane, draw: u64) -> Address {
    Address {
        stream,
        entity,
        ordinal,
        lane,
        draw,
    }
}

fn numpy_block(key: [u64; 2], counter: [u64; 4]) -> Block {
    let mut philox = PhiloxRng::new(key, counter);
    [
        philox.next_u64(),
        philox.next_u64(),
        philox.next_u64(),
        philox.next_u64(),
    ]
}

#[test]
fn philox_blocks_match_numpy() {
    // `np.random.Philox(key=key, counter=counter).random_raw(4)`.
    let cases: [([u64; 2], [u64; 4], Block); 3] = [
        (
            [0, 0],
            [0, 0, 0, 0],
            [
                0x02f4_ba64_08e4_d89b,
                0x3dd6_2b0b_9ca8_c5b2,
                0x1c86_67a5_5d90_2e79,
                0x907d_7a05_2fd5_b4dc,
            ],
        ),
        (
            [MAX, MAX],
            [MAX, MAX, MAX, MAX],
            [
                0x44b7_493d_1acf_c229,
                0x6636_af8e_9979_21dd,
                0x3f73_e132_b5b3_780e,
                0x6056_44dd_e03b_01b1,
            ],
        ),
        (
            // The pre-block increment carries out of the first word.
            [42, Stream::Mutation.code()],
            [MAX, 7, 0, 0],
            [
                0x14e2_388f_3bd4_df77,
                0xc881_fe54_e952_0ac5,
                0xf4dd_b876_c0ca_1846,
                0x9a53_310c_8525_1ecf,
            ],
        ),
    ];
    for (key, counter, expected) in cases {
        assert_eq!(numpy_block(key, counter), expected, "{key:x?} {counter:x?}");
    }
}

#[test]
fn semantic_blocks_match_numpy() {
    let rng = SemanticRng::new(42);
    let weight = lane(3, LaneKind::Weight, neuron(1, 5, 17));
    let founder = lane(0, LaneKind::Weight, neuron(0, 3, 101));
    let cases = [
        (
            rng,
            address(Stream::Mutation, 7, 1234, weight, 0),
            [
                0x68e9_8c28_72e2_b65e,
                0xb668_017d_6ab2_b781,
                0x6481_22c9_da02_c3dc,
                0xfacb_81aa_ce00_c9fc,
            ],
        ),
        (
            rng,
            address(Stream::Mutation, 7, 1234, weight, 1),
            [
                0xc4a3_351c_07a1_9c94,
                0x906c_e4a5_aa94_459a,
                0xe500_b2ee_68b8_e0a3,
                0x8e0f_e443_43b1_e03c,
            ],
        ),
        (
            rng,
            address(Stream::Founders, 1, 0, founder, 0),
            [
                0x7cf8_385e_9e07_37d2,
                0x5443_b415_e7ec_8ab4,
                0x7f08_ea3d_24fc_91f4,
                0xa8e6_4ed8_9bc5_7348,
            ],
        ),
        (
            SemanticRng::new(43),
            address(Stream::Founders, 1, 0, founder, 0),
            [
                0x5901_55af_9828_0248,
                0x517b_845c_778b_ba6a,
                0x88ae_7210_82eb_ff62,
                0xf680_7790_2c0e_5788,
            ],
        ),
        (
            rng,
            address(
                Stream::SourceChemistry,
                5,
                0,
                lane(0, LaneKind::Direction, 9),
                0,
            ),
            [
                0xd08c_1a95_4954_47c5,
                0xed17_b867_ed5f_96fe,
                0x8063_c699_c5ef_fedf,
                0x5da8_e339_f71b_9be6,
            ],
        ),
        (
            // FL-011: parent 7's reproduction attempt at tick 1234.
            rng,
            address(Stream::Birth, 7, 1234, lane(0, LaneKind::Attempt, 0), 0),
            [
                0x3f4f_7194_32ff_9508,
                0x92b2_47a0_96ff_7764,
                0x8399_704a_ba04_8e1f,
                0xfe29_b71d_8efe_9aae,
            ],
        ),
        (
            // FL-011: the same attempt's child offset.
            rng,
            address(Stream::Birth, 7, 1234, lane(0, LaneKind::Position, 0), 0),
            [
                0xa073_9383_ebe0_9bdd,
                0x6ed2_9dff_bff5_db7e,
                0x7893_1cf5_f13c_805f,
                0x9e5c_6c9c_ba17_2d0b,
            ],
        ),
    ];
    for (rng, at, expected) in cases {
        assert_eq!(rng.block(at), expected, "{at:?}");
    }
    // The oracle packed these lanes independently.
    assert_eq!(weight.get(), 0x0302_0001_0005_0011);
    assert_eq!(founder.get(), 0x0002_0000_0003_0065);
    assert_eq!(lane(0, LaneKind::Attempt, 0).get(), 0x000a_0000_0000_0000);
    assert_eq!(lane(0, LaneKind::Position, 0).get(), 0x0009_0000_0000_0000);
}

#[test]
fn stream_codes_are_frozen_tags() {
    let codes: Vec<u64> = Stream::ALL.iter().map(|stream| stream.code()).collect();
    assert_eq!(
        codes,
        [
            0x666f_756e_6465_7273, // founders
            0x6d75_7461_7469_6f6e, // mutation
            0x7372_6363_6865_6d73, // srcchems
            0x7372_6370_6174_6873, // srcpaths
            0x7372_6370_6c61_6365, // srcplace
            0x6269_7274_6869_6e67, // birthing
        ]
    );
    for stream in Stream::ALL {
        assert_eq!(stream.code().to_be_bytes(), *stream.tag());
    }
}

#[test]
fn lanes_pack_injectively_and_refuse_overflow() {
    let kinds = [
        LaneKind::Topology,
        LaneKind::Weight,
        LaneKind::Bias,
        LaneKind::MapWeight,
        LaneKind::Affinity,
        LaneKind::MotionResponse,
        LaneKind::SensingScale,
        LaneKind::Direction,
        LaneKind::Position,
        LaneKind::Attempt,
    ];
    let codes: HashSet<u64> = kinds.iter().map(|kind| kind.code()).collect();
    assert_eq!(codes.len(), kinds.len());
    let edge = (1 << 48) - 1;
    let mut seen = HashSet::new();
    for candidate in [0, 1, 7, 255] {
        for kind in kinds {
            for index in [0, 1, 0xffff, 1 << 47, edge] {
                let packed = lane(candidate, kind, index).get();
                assert_eq!(
                    packed,
                    (u64::from(candidate) << 56) | (kind.code() << 48) | index
                );
                assert!(seen.insert(packed), "{candidate} {kind:?} {index}");
            }
        }
    }
    assert!(matches!(
        Lane::new(0, LaneKind::Weight, 1 << 48),
        Err(CoreError::Overflow { .. })
    ));
    assert_eq!(neuron(2, 127, 103), (2 << 32) | (127 << 16) | 103);
    assert_eq!(neuron(255, 0xffff, 0xffff), 0xff_ffff_ffff);
    for (layer, output, input) in [(256, 0, 0), (0, 1 << 16, 0), (0, 0, 1 << 16)] {
        assert!(matches!(
            Lane::neuron_index(layer, output, input),
            Err(CoreError::Overflow { .. })
        ));
    }
}

/// A grid of addresses that differ in every coordinate.
fn grid() -> Vec<Address> {
    let mut addresses = Vec::new();
    for stream in Stream::ALL {
        for entity in [0, 1, 2, MAX] {
            for ordinal in [0, 1, 1234] {
                for candidate in [0, 7] {
                    for (kind, index) in [(LaneKind::Weight, neuron(1, 5, 17)), (LaneKind::Bias, 0)]
                    {
                        for draw in [0, 1] {
                            addresses.push(address(
                                stream,
                                entity,
                                ordinal,
                                lane(candidate, kind, index),
                                draw,
                            ));
                        }
                    }
                }
            }
        }
    }
    addresses
}

#[test]
fn blocks_depend_only_on_the_address_never_on_order_or_worker() {
    let rng = SemanticRng::new(42);
    let addresses = grid();
    let forward: BTreeMap<Address, Block> =
        addresses.iter().map(|&at| (at, rng.block(at))).collect();
    let backward: BTreeMap<Address, Block> = addresses
        .iter()
        .rev()
        .map(|&at| (at, rng.block(at)))
        .collect();
    assert_eq!(forward, backward);

    // Four workers take interleaved slices, as a scheduler might.
    let mut parallel = BTreeMap::new();
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|worker| {
                let addresses = &addresses;
                scope.spawn(move || {
                    addresses
                        .iter()
                        .skip(worker)
                        .step_by(4)
                        .map(|&at| (at, rng.block(at)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for worker in workers {
            if let Ok(pairs) = worker.join() {
                parallel.extend(pairs);
            }
        }
    });
    assert_eq!(parallel, forward);

    // Distinct addresses never share a block, and another seed moves every
    // block.
    let blocks: HashSet<Block> = forward.values().copied().collect();
    assert_eq!(blocks.len(), addresses.len());
    let other = SemanticRng::new(43);
    for (&at, block) in &forward {
        assert_ne!(&other.block(at), block, "{at:?}");
    }
}

#[test]
fn uniform_and_choice_conventions_are_exact() {
    let unit = 2.0_f64.powi(-52);
    assert_eq!(half_open_unit(0), 0.0);
    assert_eq!(half_open_unit(MAX), 1.0 - unit);
    assert_eq!(open_unit(0), unit / 2.0);
    assert_eq!(open_unit(MAX), 1.0 - unit / 2.0);
    // Only the leading 52 bits count.
    assert_eq!(half_open_unit(0xfff), 0.0);
    assert_eq!(open_unit(0x8000_0000_0000_0fff), 0.5 + unit / 2.0);
    for word in [0, 1 << 63, MAX] {
        assert!(!bernoulli(word, 0.0));
        assert!(bernoulli(word, 1.0));
    }
    assert!(bernoulli(1 << 62, 0.25 + unit));
    assert!(!bernoulli(1 << 62, 0.25));
    assert_eq!(choose(0, 4), Some(0));
    assert_eq!(choose(MAX, 4), Some(3));
    assert_eq!(choose(MAX, 1), Some(0));
    assert_eq!(choose(1, 0), None);
    // `k / 2^52` crosses `j / 3` exactly where integer arithmetic says.
    let third = (1_u64 << 52).div_ceil(3);
    assert_eq!(choose((third - 1) << 12, 3), Some(0));
    assert_eq!(choose(third << 12, 3), Some(1));
}

/// `(first, second, our ρ cos 2πu₂, our ρ sin 2πu₂, correctly rounded
/// cos, correctly rounded sin)`. Our bits are the cross-host guard: pinned
/// on macOS arm64 and checked again on Linux x86_64.
const BOX_MULLER: [(u64, u64, u64, u64, u64, u64); 3] = [
    (
        0,
        0,
        0x4021_24b2_800e_da48,
        0x0000_0000_0000_0000,
        0x4021_24b2_800e_da48,
        0x0000_0000_0000_0000,
    ),
    (
        MAX,
        MAX,
        0x3e50_0000_0000_0000,
        0xbb39_21fb_5444_2d18,
        0x3e50_0000_0000_0000,
        0xbb39_21fb_5444_2d18,
    ),
    (
        0xc4a3_351c_07a1_9c94,
        0x906c_e4a5_aa94_459a,
        0xbfe5_617d_de7c_2d8f,
        0xbfd2_3cde_d155_ebf5,
        0xbfe5_617d_de7c_2d90,
        0xbfd2_3cde_d155_ebf6,
    ),
];

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn pair(first: u64, second: u64) -> [f64; 2] {
    box_muller(first, second).expect("words always give a pair")
}

#[test]
fn box_muller_pairs_are_within_four_ulp_of_correctly_rounded() {
    for (first, second, our_cos, our_sin, cos, sin) in BOX_MULLER {
        let [ours_cos, ours_sin] = pair(first, second);
        assert_eq!(
            (ours_cos.to_bits(), ours_sin.to_bits()),
            (our_cos, our_sin),
            "{first:#x}"
        );
        assert!(ulps(ours_cos, f64::from_bits(cos)) <= 4, "{first:#x}");
        assert!(ulps(ours_sin, f64::from_bits(sin)) <= 4, "{first:#x}");
    }
    // A quarter turn: cosine is exactly zero and the sine is `ρ = √(2 ln 2)`.
    let [cos, sin] = pair(1 << 63, 1 << 62);
    assert_eq!(cos, 0.0);
    assert!(ulps(sin, f64::from_bits(0x3ff2_d6ab_e44a_fc42)) <= 2);
    // The largest radius: `u₁ = 2^-53`.
    assert!(pair(0, 0)[0] < 8.58);
}

#[test]
fn box_muller_draws_are_standard_normal() {
    let rng = SemanticRng::new(42);
    let count = 200_000_u32;
    let (mut sum, mut squares, mut cross, mut within) = (0.0, 0.0, 0.0, 0_u32);
    for entity in 0..u64::from(count / 2) {
        let at = address(
            Stream::Mutation,
            entity,
            9,
            lane(0, LaneKind::Affinity, 0),
            0,
        );
        let [w0, w1, ..] = rng.block(at);
        let [a, b] = pair(w0, w1);
        sum += a + b;
        squares += a * a + b * b;
        cross += a * b;
        within += u32::from(a.abs() < 1.0) + u32::from(b.abs() < 1.0);
    }
    let n = f64::from(count);
    let mean = sum / n;
    let variance = squares / n - mean * mean;
    let correlation = cross / (n / 2.0);
    let inside = f64::from(within) / n;
    eprintln!("mean {mean:e} variance {variance} correlation {correlation:e} inside {inside}");
    // Five standard errors each.
    assert!(mean.abs() < 5.0 / n.sqrt());
    assert!((variance - 1.0).abs() < 5.0 * (2.0 / n).sqrt());
    assert!(correlation.abs() < 5.0 / (n / 2.0).sqrt());
    // P(|Z| < 1) = erf(1/√2).
    let expected = 0.682_689_492_137_085_9;
    assert!((inside - expected).abs() < 5.0 * (expected * (1.0 - expected) / n).sqrt());
}

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

fn appendix_a_toml() -> &'static str {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    &appendix[start..start + length]
}

#[test]
fn the_rng_family_and_seed_come_from_the_configuration() {
    let config = ValidatedConfig::from_toml(appendix_a_toml()).expect("Appendix A is valid");
    let rng = SemanticRng::from_config(&config).expect("Appendix A names this family");
    assert_eq!(rng, SemanticRng::new(42));
    assert_eq!(config.config().mutation.rng, FNP_PHILOX_SEMANTIC_V1);

    let renamed = appendix_a_toml().replace(
        "rng = \"fnp-philox-semantic-v1\"",
        "rng = \"fnp-philox-semantic-v2\"",
    );
    // The registry already refuses another family at validation;
    // `from_config` refuses it again for a configuration built otherwise.
    assert_ne!(renamed, appendix_a_toml());
    assert!(ValidatedConfig::from_toml(&renamed).is_err());
}
