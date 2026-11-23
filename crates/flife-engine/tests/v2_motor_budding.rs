//! V2-05 (`so2-motor`) and V2-06 (`so2-budding`) engine tests.
//!
//! Covers:
//! - `Motion::acceleration_v2`: 0-ULP zero fast-path vs `Motion::acceleration`,
//!   SO(2) rotation equivariance (`<= 1e-13`), `v = 0` and `g_sum = 0` heading
//!   fallbacks, post-maintenance `E / (E + E_s)` propulsion scaling, and
//!   validation refusals.
//! - `Reproduction::budding_child_offset`: `BuddingSteering::ZERO` disk bound
//!   (`<= r_local`), SO(2) rotation equivariance (`<= 1e-13`), `∇q = 0` / `v = 0`
//!   Philox word-2 fallback, `λ_epi` composition with `V2-08` lateral mitosis,
//!   and validation refusals.
//! - Full `World` E2E tests for `so2-motor` and `so2-budding`: `--disable`
//!   bit-exact parity with `--v1-compat`, `1 vs 4` CPU workers, `C.7`
//!   (`Compact` vs `FreeList`), `C.8` candidate cancel, checkpoint v3
//!   round-trip, `World::watch` + `organism_snapshot` trace consistency, and
//!   `field-ecology-v2-chemistry-only`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration tests assert directly on invariants and fixtures"
)]

use std::num::NonZeroUsize;

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::ids::OrganismId;
use flife_core::rng::SemanticRng;
use flife_core::{FeatureOverride, Tick, ValidatedConfig};
use flife_engine::{
    Advance, Arrangement, Motion, MotionResponse, NumericalProfile, RESPONSE_TERMS, Reproduction,
    Stepped, TickReport, WORLD_CHECKPOINT_CHUNKS, World,
};
use flife_field::settle::{MotorSteeringConstants, MotorSteeringInputs};
use flife_neural::BuddingSteering;

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

fn ecology_toml(model: &str, overrides: &[(&str, &str, &str)], tail: &str) -> String {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    let model_line = lines
        .iter()
        .position(|line| line.starts_with("model = "))
        .expect("model line");
    lines[model_line] = format!("model = \"{model}\"");
    for &(section, key, value) in overrides {
        let header = format!("[{section}]");
        let first = lines
            .iter()
            .position(|line| line.trim() == header)
            .expect("section");
        let line = lines[first + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| first + 1 + offset)
            .expect("key");
        lines[line] = format!("{key} = {value}");
    }
    let mut out = lines.join("\n");
    if !tail.is_empty() {
        out.push('\n');
        out.push_str(tail);
    }
    out
}

fn small_overrides() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        ("world", "width", "16.0"),
        ("world", "height", "16.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("world", "tick_numerator", "1"),
        ("world", "tick_denominator", "32"),
        ("population", "initial_count", "32"),
        ("population", "maximum_count", "128"),
        ("population", "initial_energy", "4.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "250.0"),
        ("sources", "sigma_world_units", "3.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.25"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.0",
        ),
        ("reproduction", "maximum_attempt_rate_per_second", "6.0"),
        ("reproduction", "cooldown_seconds", "0.125"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ]
}

fn config_with(model: &str, tail: &str, feature_overrides: &[FeatureOverride]) -> ValidatedConfig {
    let toml = ecology_toml(model, small_overrides(), tail);
    ValidatedConfig::from_toml_with_overrides(&toml, feature_overrides)
        .expect("v2 test config validates")
}

fn step_published(world: &mut World) -> Box<TickReport> {
    match world.step().expect("step succeeds") {
        Stepped::Published(report) => report,
        Stepped::Paused(shortfall) => panic!("unexpected capacity pause: {shortfall:?}"),
    }
}

fn rotate(v: [f64; 2], angle: f64) -> [f64; 2] {
    let (s, c) = angle.sin_cos();
    [c * v[0] - s * v[1], s * v[0] + c * v[1]]
}

#[test]
fn so2_motor_zero_fast_path_equivariance_fallbacks_and_energy_gate() {
    let cfg = config_with(
        "field-ecology-v2",
        "",
        &[
            FeatureOverride::V1Compat,
            FeatureOverride::Enable("so2-motor".to_owned()),
        ],
    );
    let motion = Motion::from_config(&cfg).expect("motion");
    let constants = MotorSteeringConstants {
        energy_gradient_gain: 1.5,
        chemical_gradient_gain: 1.25,
        chiral_contour_gain: 0.75,
        heading_propulsion_fraction: 0.4,
        internal_energy_scale: 2.0,
    };

    let mut coeffs = [0.0_f64; RESPONSE_TERMS];
    let mut gradients = [[0.0_f64; 2]; RESPONSE_TERMS];
    for i in 0..RESPONSE_TERMS {
        let f = (i + 1) as f64;
        coeffs[i] = 0.05 * ((i % 5) as f64 - 2.0);
        gradients[i] = [0.12 * (f * 0.37).cos(), -0.09 * (f * 0.53).sin()];
    }
    let response = MotionResponse::new(coeffs, &motion).expect("response");
    let velocity = [0.35, -0.22];

    // 1. Exact 0-ULP baseline fast-path when motor is zero.
    let baseline = motion
        .acceleration(0.65, &response, &gradients)
        .expect("baseline acceleration");
    let v2_zero = motion
        .acceleration_v2(
            0.65,
            &response,
            &gradients,
            velocity,
            3.5,
            &constants,
            MotorSteeringInputs::default(),
        )
        .expect("zero motor acceleration");
    assert_eq!(baseline[0].to_bits(), v2_zero[0].to_bits());
    assert_eq!(baseline[1].to_bits(), v2_zero[1].to_bits());

    // 2. SO(2) equivariance across 90°, 180°, 270°, and 37°.
    let active_motor = MotorSteeringInputs {
        energy_gradient: 0.6,
        chemical_gradient: -0.45,
        chiral_contour: 0.8,
        heading_propulsion: 0.7,
    };
    let a0 = motion
        .acceleration_v2(
            0.75,
            &response,
            &gradients,
            velocity,
            4.0,
            &constants,
            active_motor,
        )
        .expect("active motor acceleration");
    for degrees in [90.0_f64, 180.0, 270.0, 37.0] {
        let theta = degrees.to_radians();
        let rot_gradients = gradients.map(|g| rotate(g, theta));
        let rot_velocity = rotate(velocity, theta);
        let a_rot = motion
            .acceleration_v2(
                0.75,
                &response,
                &rot_gradients,
                rot_velocity,
                4.0,
                &constants,
                active_motor,
            )
            .expect("rotated acceleration");
        let expected = rotate(a0, theta);
        let err = (a_rot[0] - expected[0]).hypot(a_rot[1] - expected[1]);
        assert!(
            err <= 1.0e-13,
            "SO(2) equivariance error {err:e} at {degrees} deg exceeded 1e-13"
        );
    }

    // 3. Heading fallbacks (`v = 0` -> `g_sum / ||g_sum||`; `v = 0 && g_sum = 0` -> `[0, 0]`).
    let prop_only = MotorSteeringInputs {
        energy_gradient: 0.0,
        chemical_gradient: 0.0,
        chiral_contour: 0.0,
        heading_propulsion: 1.0,
    };
    let mut grad_only_x = [[0.0_f64; 2]; RESPONSE_TERMS];
    grad_only_x[0] = [0.1, 0.0];
    let mut chi_only_e = [0.0_f64; RESPONSE_TERMS];
    chi_only_e[0] = 0.25;
    let resp_e = MotionResponse::new(chi_only_e, &motion).expect("resp_e");
    let a_from_grad = motion
        .acceleration_v2(
            0.5,
            &resp_e,
            &grad_only_x,
            [0.0, 0.0],
            2.0,
            &constants,
            prop_only,
        )
        .expect("v=0 fallback");
    assert!(a_from_grad[0] > 0.0 && a_from_grad[1].abs() <= 1.0e-15);

    let zero_grads = [[0.0_f64; 2]; RESPONSE_TERMS];
    let a_all_zero = motion
        .acceleration_v2(
            0.5,
            &resp_e,
            &zero_grads,
            [0.0, 0.0],
            2.0,
            &constants,
            prop_only,
        )
        .expect("v=0 g_sum=0 fallback");
    assert_eq!(a_all_zero, [0.0, 0.0]);

    // 4. Post-maintenance energy gate `remaining / (remaining + E_s)` monotonicity.
    let a_zero_e = motion
        .acceleration_v2(
            0.0,
            &resp_e,
            &zero_grads,
            [1.0, 0.0],
            0.0,
            &constants,
            prop_only,
        )
        .expect("remaining=0");
    let a_low_e = motion
        .acceleration_v2(
            0.0,
            &resp_e,
            &zero_grads,
            [1.0, 0.0],
            1.0,
            &constants,
            prop_only,
        )
        .expect("remaining=1");
    let a_high_e = motion
        .acceleration_v2(
            0.0,
            &resp_e,
            &zero_grads,
            [1.0, 0.0],
            8.0,
            &constants,
            prop_only,
        )
        .expect("remaining=8");
    assert_eq!(a_zero_e, [0.0, 0.0]);
    assert!(a_low_e[0] > 0.0 && a_high_e[0] > a_low_e[0]);
}

#[test]
fn so2_budding_unit_equivariance_fallbacks_and_epi_lock() {
    let cfg = config_with(
        "field-ecology-v2",
        "",
        &[
            FeatureOverride::V1Compat,
            FeatureOverride::Enable("so2-budding".to_owned()),
        ],
    );
    let repro = Reproduction::from_config(&cfg).expect("reproduction");
    let rng = SemanticRng::new(77);
    let parent = OrganismId::new(11);
    let tick = Tick::new(23);
    let r_max = repro.rules().offset_radius;
    let r_local = 0.25_f64.min(0.5 * r_max);

    // 1. Zero steering stays inside `r_local` disk.
    let base = repro
        .budding_child_offset(
            &rng,
            parent,
            tick,
            Some(r_local),
            Some(BuddingSteering::ZERO),
            [0.5, -0.3],
            [0.1, 0.2],
            0.0,
            [0.0, 0.0],
        )
        .expect("zero steering");
    let base_norm = base[0].hypot(base[1]);
    assert!(
        base_norm <= r_local + 1.0e-12,
        "zero steering norm {base_norm} exceeded r_local {r_local}"
    );

    // 2. Active steering obeys SO(2) rotation equivariance on the steered displacement.
    let steer = BuddingSteering::decode([0.9, -0.45]).expect("budding steering");
    let grad = [0.6, -0.25];
    let vel = [-0.2, 0.4];
    let off0 = repro
        .budding_child_offset(
            &rng,
            parent,
            tick,
            Some(r_local),
            Some(steer),
            grad,
            vel,
            0.0,
            [0.0, 0.0],
        )
        .expect("active steering");
    let delta0 = [off0[0] - base[0], off0[1] - base[1]];
    assert!(delta0[0].hypot(delta0[1]) > 0.0);
    for degrees in [90.0_f64, 180.0, 270.0, 37.0] {
        let theta = degrees.to_radians();
        let off_rot = repro
            .budding_child_offset(
                &rng,
                parent,
                tick,
                Some(r_local),
                Some(steer),
                rotate(grad, theta),
                rotate(vel, theta),
                0.0,
                [0.0, 0.0],
            )
            .expect("rotated steering");
        let delta_rot = [off_rot[0] - base[0], off_rot[1] - base[1]];
        let expected = rotate(delta0, theta);
        let err = (delta_rot[0] - expected[0]).hypot(delta_rot[1] - expected[1]);
        assert!(
            err <= 1.0e-13,
            "budding SO(2) equivariance error {err:e} at {degrees} deg exceeded 1e-13"
        );
    }

    // 3. `grad = 0` falls back to `velocity`, and `grad = v = 0` uses deterministic Philox word 2.
    let off_vel = repro
        .budding_child_offset(
            &rng,
            parent,
            tick,
            Some(r_local),
            Some(steer),
            [0.0, 0.0],
            [0.0, 2.0],
            0.0,
            [0.0, 0.0],
        )
        .expect("velocity fallback");
    assert!(off_vel[0].is_finite() && off_vel[1].is_finite());

    let off_philox_a = repro
        .budding_child_offset(
            &rng,
            parent,
            tick,
            Some(r_local),
            Some(steer),
            [0.0, 0.0],
            [0.0, 0.0],
            0.0,
            [0.0, 0.0],
        )
        .expect("philox fallback a");
    let off_philox_b = repro
        .budding_child_offset(
            &rng,
            parent,
            tick,
            Some(r_local),
            Some(steer),
            [0.0, 0.0],
            [0.0, 0.0],
            0.0,
            [0.0, 0.0],
        )
        .expect("philox fallback b");
    assert_eq!(off_philox_a[0].to_bits(), off_philox_b[0].to_bits());
    assert_eq!(off_philox_a[1].to_bits(), off_philox_b[1].to_bits());
    assert!(off_philox_a[0].hypot(off_philox_a[1]) <= r_max + 1.0e-12);

    // 4. `λ_epi` composition with V2-08 lateral mitosis.
    let lateral = [0.42, -0.17];
    let locked = repro
        .budding_child_offset(
            &rng,
            parent,
            tick,
            Some(r_local),
            Some(steer),
            grad,
            vel,
            1.0,
            lateral,
        )
        .expect("full epi lock");
    assert_eq!(locked[0].to_bits(), lateral[0].to_bits());
    assert_eq!(locked[1].to_bits(), lateral[1].to_bits());

    // 5. Validation guards.
    assert!(
        repro
            .budding_child_offset(
                &rng,
                parent,
                tick,
                Some(r_max + 0.1),
                Some(steer),
                grad,
                vel,
                0.0,
                lateral
            )
            .is_err()
    );
    assert!(
        repro
            .budding_child_offset(
                &rng,
                parent,
                tick,
                Some(r_local),
                Some(steer),
                grad,
                vel,
                1.5,
                lateral
            )
            .is_err()
    );
}

#[test]
fn so2_motor_and_budding_disabled_match_v1_compat_bit_exact() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let base_cfg = config_with("field-ecology-v2", "", &[FeatureOverride::V1Compat]);
    let toggled_cfg = config_with(
        "field-ecology-v2",
        "",
        &[
            FeatureOverride::V1Compat,
            FeatureOverride::Enable("so2-motor".to_owned()),
            FeatureOverride::Enable("so2-budding".to_owned()),
            FeatureOverride::Disable("so2-motor".to_owned()),
            FeatureOverride::Disable("so2-budding".to_owned()),
        ],
    );
    let mut base = World::found(&base_cfg, Arrangement::Compact, profile).expect("base world");
    let mut toggled =
        World::found(&toggled_cfg, Arrangement::Compact, profile).expect("toggled world");
    assert_eq!(base.footprint(), toggled.footprint());
    for _ in 0..24 {
        step_published(&mut base);
        step_published(&mut toggled);
    }
    assert_eq!(base.digest(), toggled.digest());
}

#[test]
fn so2_motor_and_budding_e2e_workers_c7_c8_checkpoint_watch_and_chemistry_only() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let cfg = config_with(
        "field-ecology-v2",
        "",
        &[
            FeatureOverride::V1Compat,
            FeatureOverride::Enable("so2-motor".to_owned()),
            FeatureOverride::Enable("so2-budding".to_owned()),
        ],
    );
    let mut w1 = World::found_on(
        &cfg,
        Arrangement::Compact,
        profile,
        NonZeroUsize::new(1).expect("1"),
    )
    .expect("w1");
    let mut w4 = World::found_on(
        &cfg,
        Arrangement::FreeList,
        profile,
        NonZeroUsize::new(4).expect("4"),
    )
    .expect("w4");
    let watched_id = w1.population().ids().next().expect("founder 0");
    w1.watch(Some(watched_id));

    let mut total_births = 0usize;
    for tick in 0..36 {
        if tick == 18 {
            // C.8 candidate cancel must leave committed state untouched.
            let digest_before = w1.digest();
            if let Advance::Ready(candidate) = w1.advance().expect("cancel advance") {
                candidate.cancel();
            }
            assert_eq!(w1.digest(), digest_before);
        }
        let report = step_published(&mut w1);
        total_births += report.births.len();
        step_published(&mut w4);
        assert_eq!(
            w1.digest(),
            w4.digest(),
            "C.7 / 1v4 worker parity at tick {tick}"
        );
    }
    assert!(
        total_births > 0,
        "expected births under active so2-motor + so2-budding"
    );

    // Watch / inspector trace consistency across expanded 42-output action layout.
    if let Some(first_live) = w1.population().iter().next() {
        let snap = w1
            .organism_snapshot(first_live.id)
            .expect("organism snapshot")
            .expect("live snapshot");
        let decoded = snap.decoded.expect("decoded actions");
        assert!(decoded.motor.is_some());
        assert!(decoded.budding.is_some());
    }

    // Checkpoint v3 round-trip bit-exactness.
    let mut writer = CheckpointWriter::new(&cfg.config().model);
    w1.write_checkpoint(&mut writer).expect("write checkpoint");
    let bytes = writer.finish().expect("finish checkpoint");
    let reader =
        CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("open checkpoint");
    let mut restored = World::read_checkpoint(&reader, profile).expect("read checkpoint");
    assert_eq!(restored.digest(), w1.digest());
    for _ in 0..12 {
        step_published(&mut w1);
        step_published(&mut restored);
    }
    assert_eq!(restored.digest(), w1.digest());

    // `field-ecology-v2-chemistry-only` zeroes motor & budding outputs cleanly.
    let chem_cfg = config_with(
        "field-ecology-v2-chemistry-only",
        "",
        &[
            FeatureOverride::V1Compat,
            FeatureOverride::Enable("so2-motor".to_owned()),
            FeatureOverride::Enable("so2-budding".to_owned()),
        ],
    );
    let mut chem = World::found(&chem_cfg, Arrangement::Compact, profile).expect("chem-only world");
    for _ in 0..16 {
        step_published(&mut chem);
    }
    assert!(chem.ledger().report().residual.abs() <= profile.residual.cumulative());
}
