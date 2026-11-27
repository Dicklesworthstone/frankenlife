//! FL-045 evidence (plan §19.5, §21.3 Gate G7): `presets/screenshot-world.toml`
//! is FrankenLife's full-evaluation 400k Screenshot-scale operating profile
//! preset (`400,000` founders, `1024 × 1024 × 33` periodic field, `F16`
//! canonical weights, `evaluation_interval_ticks = 1`, `640,000` ceiling).
//!
//! These tests enforce the FL-045 preset contract:
//! 1. The preset is `presets/desktop-world.toml` with ONLY its 11 declared
//!    scale/source/memory changes, each named in its header.
//! 2. The preset validates at the full 400k Screenshot-scale profile (including
//!    mean neural size inside G7's `[9_000, 11_000]` band, full-evaluation
//!    cadence `K = 1`, `pause-before-commit` explicit capacity policy, and
//!    `automatic_reseed = false`), and its session + checkpoint preflight admit
//!    it cleanly.
//! 3. Compact scaled physics execution reproduces bit-exact `WorldDigest`
//!    parity across `cpu` and `metal` (where available) with zero deferrals,
//!    strict `INV-03` conservation, and full per-tick evaluation count.

#![forbid(unsafe_code)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests: an unexpected failure is a test failure"
)]

mod preset_diff;

use std::num::NonZeroUsize;

use flife_backend_metal::{ControllerChoice, MetalControllers};
use flife_cli::run::configure;
use flife_cli::{CheckpointMode, RUN_SESSION, VALIDATE_SCHEMA, Validation, admit};
use flife_core::ValidatedConfig;
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_runtime::SessionConfig;
use preset_diff::assert_exactly_declared;

const PRESET: &str = include_str!("../../../presets/screenshot-world.toml");
const DESKTOP: &str = include_str!("../../../presets/desktop-world.toml");

/// The preset's declared changes from the Desktop ecology preset, as TOML
/// values: `(key, desktop-world value, screenshot-world value)`.
const DECLARED: [(&str, &str, &str); 11] = [
    ("name", "\"desktop-world\"", "\"screenshot-world\""),
    ("world.width", "512.0", "1024.0"),
    ("world.height", "512.0", "1024.0"),
    ("world.grid_width", "512", "1024"),
    ("world.grid_height", "512", "1024"),
    ("population.initial_count", "100000", "400000"),
    ("population.maximum_count", "160000", "640000"),
    ("sources.count", "128", "512"),
    (
        "execution.maximum_logical_genome_bytes",
        "4294967296",
        "17179869184",
    ),
    (
        "execution.maximum_allocated_genome_bytes",
        "4831838208",
        "19327352832",
    ),
    (
        "execution.maximum_total_bytes",
        "17179869184",
        "68719476736",
    ),
];

/// The preset is `presets/desktop-world.toml` with exactly its declared
/// changes, and its header declares each one.
#[test]
fn the_preset_is_the_desktop_preset_with_exactly_its_declared_changes() {
    assert_exactly_declared(DESKTOP, PRESET, &DECLARED);
}

/// The preset validates at the full-evaluation 400k Screenshot-scale profile
/// (plan §19.5, §21.3 Gate G7), and both the session and checkpoint preflight
/// admit it cleanly.
#[test]
fn the_preset_validates_and_admits_at_the_screenshot_400k_scale() {
    let config = ValidatedConfig::from_toml(PRESET).expect("screenshot-world preset validates");
    let cfg = config.config();
    assert_eq!(cfg.controller.evaluation_interval_ticks, 1);
    assert_eq!(cfg.controller.canonical_weight_dtype, "f16");
    assert_eq!(cfg.population.capacity_policy, "pause-before-commit");
    assert!(!cfg.population.automatic_reseed);

    let summary = Validation::of(&config);
    assert_eq!(summary.schema, VALIDATE_SCHEMA);
    assert_eq!(summary.name, "screenshot-world");
    assert_eq!(summary.model, "field-ecology-v1");
    assert_eq!(summary.seed, 42);
    assert_eq!(summary.grid, [1024, 1024]);
    assert_eq!(summary.founders, 400_000);
    assert_eq!(summary.maximum_population, 640_000);
    assert_eq!(summary.timestep, [1, 128]);
    assert_eq!(summary.default_parameters, 9988);
    assert!(
        (9_000..=11_000).contains(&summary.default_parameters),
        "default parameter count {} must lie inside Gate G7's 9,000..=11,000 band",
        summary.default_parameters
    );
    // F16 canonical weights: 2 bytes per parameter across 400,000 founders (~7.99 GB).
    assert_eq!(summary.initial_genome_bytes, 400_000 * 9988 * 2);
    assert_eq!(summary.command_durability, "volatile-in-memory");

    // Area-normalized source power matches desktop-world and interactive-world.
    let area = cfg.world.width * cfg.world.height;
    let power_per_area = f64::from(cfg.sources.count) * cfg.sources.power_per_source / area;
    let desktop_cfg = ValidatedConfig::from_toml(DESKTOP).expect("desktop validates");
    let dcfg = desktop_cfg.config();
    let desktop_power_per_area = f64::from(dcfg.sources.count) * dcfg.sources.power_per_source
        / (dcfg.world.width * dcfg.world.height);
    assert!((power_per_area - desktop_power_per_area).abs() < 1e-12);

    assert!(SessionConfig::from_config(RUN_SESSION, &config).is_ok());
    let admission = admit(
        &config,
        CheckpointMode::PauseAndStream,
        cfg.execution.checkpoint_retained_bytes,
    );
    assert!(
        admission.is_ok(),
        "pause-and-stream admission failed: {admission:?}"
    );
}

/// Compact scaled execution of `presets/screenshot-world.toml` verifies full
/// per-tick neural evaluation (`K = 1`), `INV-03` conservation, and bit-exact
/// `cpu` vs `metal` parity with zero deferrals.
#[test]
fn screenshot_world_full_evaluation_compact_parity_and_invariants() {
    let mut text = PRESET.to_owned();
    for (from, to) in [
        ("width = 1024.0\n", "width = 64.0\n"),
        ("height = 1024.0\n", "height = 64.0\n"),
        ("grid_width = 1024\n", "grid_width = 64\n"),
        ("grid_height = 1024\n", "grid_height = 64\n"),
        ("\ninitial_count = 400000\n", "\ninitial_count = 512\n"),
        ("\nmaximum_count = 640000\n", "\nmaximum_count = 1024\n"),
        ("\ncount = 512\n", "\ncount = 8\n"),
        ("sigma_world_units = 10.0\n", "sigma_world_units = 4.0\n"),
        (
            "maximum_logical_genome_bytes = 17179869184\n",
            "maximum_logical_genome_bytes = 268435456\n",
        ),
        (
            "maximum_allocated_genome_bytes = 19327352832\n",
            "maximum_allocated_genome_bytes = 335544320\n",
        ),
        (
            "maximum_total_bytes = 68719476736\n",
            "maximum_total_bytes = 1073741824\n",
        ),
    ] {
        assert!(text.contains(from), "preset missing expected line {from:?}");
        text = text.replacen(from, to, 1);
    }

    let config = ValidatedConfig::from_toml(&text).expect("compact screenshot preset validates");
    let profile = NumericalProfile::reference_f32().expect("reference profile");
    let workers = NonZeroUsize::new(4).expect("non-zero");
    let ticks = 16_u64;

    let mut world_cpu =
        World::found_on(&config, Arrangement::Compact, profile, workers).expect("cpu found");
    let mut total_evals_cpu = 0_u64;
    let mut expected_total_evals = 0_u64;
    for _ in 0..ticks {
        let live_before = u64::try_from(world_cpu.population().len()).expect("u64");
        let report = match world_cpu.step().expect("cpu step") {
            Stepped::Published(r) => *r,
            Stepped::Paused(shortfall) => panic!("unexpected capacity pause: {shortfall:?}"),
        };
        let evals = u64::try_from(report.evaluated).expect("u64");
        total_evals_cpu += evals;
        expected_total_evals += live_before;
        // Full-evaluation contract (K = 1): EVERY organism alive entering phase 4
        // must be evaluated on every tick (no reduced cadence shortcut).
        assert_eq!(
            evals,
            live_before,
            "K=1 full-evaluation violation at tick {}: evaluated {evals} != live_before {live_before}",
            report.tick.get()
        );
        let norm_res = report.ledger.normalized.unwrap_or(0.0).abs();
        assert!(
            norm_res <= 1.0e-5,
            "INV-03 normalized residual {norm_res} exceeded 1e-5 at tick {}",
            report.tick.get()
        );
    }
    let cpu_digest = world_cpu.digest();
    assert_eq!(total_evals_cpu, expected_total_evals);
    assert!(total_evals_cpu >= 512 * ticks);

    // If Metal is available on this host, require 0-ULP bit-exact parity and zero deferrals.
    if MetalControllers::open().is_ok() {
        let mut world_metal =
            World::found_on(&config, Arrangement::Compact, profile, workers).expect("metal found");
        configure(
            &mut world_metal,
            Some(workers),
            ControllerChoice::Metal,
            None,
        )
        .expect("configure metal");
        for _ in 0..ticks {
            let live_before = world_metal.population().len();
            match world_metal.step().expect("metal step") {
                Stepped::Published(r) => {
                    assert_eq!(r.evaluated, live_before, "Metal K=1 evaluation mismatch");
                }
                Stepped::Paused(shortfall) => {
                    panic!("unexpected Metal capacity pause: {shortfall:?}")
                }
            }
        }
        assert_eq!(
            world_metal.digest(),
            cpu_digest,
            "screenshot-world F16 Metal digest must match CPU bit-for-bit"
        );
        let ctrl = world_metal.controller_report();
        assert_eq!(ctrl.deferred, 0, "expected 0 Metal deferrals");
        assert_eq!(
            ctrl.reference_ticks, 0,
            "expected 0 fallback reference ticks"
        );
        assert!(
            ctrl.failure.is_none(),
            "unexpected Metal failure: {:?}",
            ctrl.failure
        );
        assert_eq!(
            ctrl.batched, total_evals_cpu,
            "expected Metal to batch every K=1 evaluation without drops or extras"
        );
    }
}
