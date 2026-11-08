//! FL-010's seeded sources (plan A.3 items 7 and 8, §16.2, C.17, INV-11):
//! chemical directions, initial centers and travel directions against an
//! independent oracle (NumPy 2.5.4's `Philox` and Python `Decimal` at 80
//! digits), order independence, and seeded paths that join continuously.

use std::collections::BTreeMap;

use flife_core::ids::SourceId;
use flife_core::rng::SemanticRng;
use flife_core::{TimeStep, ValidatedConfig};
use flife_field::state::CHANNELS;
use flife_field::{
    FieldGrid, SeededPath, chemical_direction, initial_center, initial_source, travel_direction,
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
fn world() -> FieldGrid {
    FieldGrid::from_config(&appendix_a()).expect("Appendix A grid")
}

fn step() -> TimeStep {
    appendix_a().report().timestep
}

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

fn squared(values: &[f64]) -> f64 {
    values
        .iter()
        .fold(0.0, |total, &value| total + value * value)
}

const RNG: SemanticRng = SemanticRng::new(42);

/// Source 5's chemical direction, correctly rounded from the oracle's 32
/// Box–Muller normals.
const SOURCE_5: [u64; CHANNELS] = [
    0xbfc4_05af_3f13_ae85,
    0x3fbd_8640_4585_66a8,
    0xbfb8_eb9a_f7b9_0289,
    0x3fc6_5fed_a521_1ba6,
    0x3f94_a87e_3329_e11f,
    0xbfc3_b9e2_94a4_0289,
    0xbfc4_2dac_a159_af4f,
    0x3fd1_86ec_e467_2c23,
    0x3fb5_887c_3a26_6de4,
    0x3fc4_6743_c983_29dc,
    0xbfd7_7ed2_f2c7_de5f,
    0x3fd6_bc4a_177d_2df4,
    0x3fba_5859_e92f_2dcb,
    0x3fa0_a31b_a905_5644,
    0xbfae_eefa_fd42_e4c2,
    0xbfae_6742_bca5_6b27,
    0xbfd6_78a3_5244_0466,
    0x3fb9_53e5_8f9b_6c55,
    0x3fbf_4376_9027_0373,
    0xbfaf_4ba2_dc94_efcd,
    0x3fc2_b841_6235_8561,
    0xbfbe_84ef_076a_63ed,
    0x3fbf_cf27_b529_6de0,
    0x3fb7_ce74_036f_ac42,
    0x3f84_99ed_52e5_4fda,
    0x3fdd_0edf_e71d_ac98,
    0x3f9c_d330_8b08_4a42,
    0x3fcb_a75b_5c8e_a03e,
    0x3fb3_462c_50c3_e50c,
    0xbfa7_812f_ec2a_eec1,
    0xbfbb_00d8_0b27_5814,
    0x3fa6_4049_9a0f_7e01,
];

#[test]
fn seeded_directions_match_an_independent_oracle() {
    let ours = chemical_direction(&RNG, SourceId::new(5)).expect("a direction");
    for (channel, (&value, &expected)) in ours.iter().zip(&SOURCE_5).enumerate() {
        let error = ulps(value, f64::from_bits(expected));
        assert!(error <= 8, "channel {channel}: {error} ulp");
    }
    let norm = squared(&ours);
    assert!(norm <= 1.0 && norm > 1.0 - 8.0 * f64::EPSILON, "{norm}");

    // Uniform by angle: source 3's direction at segment 2, from word
    // 0x25128abd6b54d7ef.
    let [x, y] = travel_direction(&RNG, SourceId::new(3), 2).expect("a direction");
    assert!(ulps(x, f64::from_bits(0x3fe3_a47d_1ab5_01a1)) <= 2);
    assert!(ulps(y, f64::from_bits(0x3fe9_4315_4e93_d3e8)) <= 2);

    // Uniform on the torus: 128 × u is exact.
    let center = initial_center(&RNG, &world(), SourceId::new(3)).expect("a center");
    assert_eq!(
        center.map(f64::to_bits),
        [0x4055_07b5_4631_9004, 0x4053_6099_22f3_c1b0]
    );
}

#[test]
fn seeded_sources_depend_only_on_the_seed_and_their_identity() {
    let world = world();
    let power = appendix_a().config().sources.power_per_source;
    let make = |ids: &mut dyn Iterator<Item = u32>| -> BTreeMap<u32, _> {
        ids.map(|id| {
            let source = initial_source(&RNG, &world, SourceId::new(id), power).expect("source");
            (id, (source.center(), *source.direction()))
        })
        .collect()
    };
    let forward = make(&mut (0..64));
    assert_eq!(make(&mut (0..64).rev()), forward);
    assert_eq!(
        make(
            &mut (0..64)
                .filter(|id| id % 2 == 0)
                .chain((0..64).filter(|id| id % 2 == 1))
        ),
        forward
    );
    let other = SemanticRng::new(43);
    let moved = initial_source(&other, &world, SourceId::new(5), power).expect("source");
    assert_ne!((moved.center(), *moved.direction()), forward[&5]);

    // Isotropy and uniformity to sampling error over 4,096 sources.
    let count = 4_096_u32;
    let mut mean = [0.0_f64; CHANNELS];
    let mut centers = [0.0_f64; 2];
    let mut angles = [0.0_f64; 2];
    for id in 0..count {
        let direction = chemical_direction(&RNG, SourceId::new(id)).expect("direction");
        for (total, value) in mean.iter_mut().zip(direction) {
            *total += value;
        }
        let center = initial_center(&RNG, &world, SourceId::new(id)).expect("center");
        assert!((0.0..128.0).contains(&center[0]) && (0.0..128.0).contains(&center[1]));
        centers[0] += center[0];
        centers[1] += center[1];
        let travel = travel_direction(&RNG, SourceId::new(id), 0).expect("travel");
        assert!((squared(&travel) - 1.0).abs() < 4.0 * f64::EPSILON);
        angles[0] += travel[0];
        angles[1] += travel[1];
    }
    let n = f64::from(count);
    for total in mean {
        assert!((total / n).abs() < 5.0 * (1.0 / 32.0 / n).sqrt());
    }
    // A uniform coordinate on [0, 128) has mean 64 and variance 128²/12.
    for total in centers {
        assert!((total / n - 64.0).abs() < 5.0 * (128.0 * 128.0 / 12.0 / n).sqrt());
    }
    // cos and sin of a uniform angle have mean 0 and variance 1/2.
    for total in angles {
        assert!((total / n).abs() < 5.0 * (0.5 / n).sqrt());
    }
}

#[test]
fn seeded_paths_chain_their_own_directions_and_join_continuously() {
    let world = world();
    let config = appendix_a();
    let sources = &config.config().sources;
    let step = step();
    let ticks = step
        .ticks_exact(sources.path_direction_interval_seconds, "interval")
        .expect("whole ticks");
    let speed = sources.path_speed;
    let id = SourceId::new(9);
    let mut path = SeededPath::start(&RNG, &world, id, speed, ticks, step).expect("segment 0");
    assert_eq!(
        path.segment().position(&world, 0.0).expect("anchor"),
        initial_center(&RNG, &world, id).expect("center")
    );
    for ordinal in 0..12_u64 {
        assert_eq!(path.ordinal(), ordinal);
        let segment = *path.segment();
        let direction = travel_direction(&RNG, id, ordinal).expect("d_k");
        let following = travel_direction(&RNG, id, ordinal + 1).expect("d_k+1");
        let at = |u| segment.velocity(u).expect("velocity").map(|v| v / speed);
        for (ours, theirs) in at(0.0).iter().zip(direction) {
            assert!((ours - theirs).abs() < 1.0e-15);
        }
        for (ours, theirs) in at(1.0).iter().zip(following) {
            assert!((ours - theirs).abs() < 1.0e-15);
        }
        for k in (0..segment.ticks()).step_by(97) {
            let u = segment.mid_tick(k).expect("inside");
            let velocity = segment.velocity(u).expect("velocity");
            assert!(squared(&velocity).sqrt() <= speed * (1.0 + 1.0e-12));
        }
        let next = path.next(&RNG, &world, step).expect("next segment");
        assert_eq!(
            next.segment().position(&world, 0.0).expect("start"),
            segment.position(&world, 1.0).expect("end")
        );
        assert_eq!(
            next.segment().velocity(0.0).expect("start"),
            segment.velocity(1.0).expect("end")
        );
        path = next;
    }

    // A path depends on nothing but its source: advancing another source's
    // path in between changes nothing, and a reseeded run moves it.
    let other = SourceId::new(10);
    let mut alone = SeededPath::start(&RNG, &world, id, speed, ticks, step).expect("start");
    let mut mixed = alone;
    let mut neighbour = SeededPath::start(&RNG, &world, other, speed, ticks, step).expect("start");
    for _ in 0..5 {
        alone = alone.next(&RNG, &world, step).expect("next");
        neighbour = neighbour.next(&RNG, &world, step).expect("next");
        mixed = mixed.next(&RNG, &world, step).expect("next");
    }
    assert_eq!(alone, mixed);
    assert_ne!(alone.segment(), neighbour.segment());
    let reseeded =
        SeededPath::start(&SemanticRng::new(43), &world, id, speed, ticks, step).expect("start");
    assert_ne!(
        reseeded.segment().position(&world, 0.0).expect("anchor"),
        SeededPath::start(&RNG, &world, id, speed, ticks, step)
            .expect("start")
            .segment()
            .position(&world, 0.0)
            .expect("anchor")
    );

    // A path update moves the center and keeps the chemistry (fixed-per-source).
    let source = initial_source(&RNG, &world, id, sources.power_per_source).expect("source");
    let moved = source
        .moved_to(path.segment().position(&world, 0.5).expect("midpoint"))
        .expect("moved");
    assert_eq!(moved.direction(), source.direction());
    assert_ne!(moved.center(), source.center());
}
