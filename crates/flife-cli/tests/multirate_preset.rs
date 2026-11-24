//! FL-046 evidence (plan §6.6, §19.5): `presets/desktop-multirate-4t.toml` is
//! FrankenLife's separately named multi-rate Desktop ecology operating profile
//! (`controller.evaluation_interval_ticks = 4`, F16 canonical weights).
//!
//! These tests enforce the full FL-046 contract:
//! 1. The preset is `presets/desktop-world.toml` with ONLY its declared name
//!    and `controller.evaluation_interval_ticks` changes, each declared in its
//!    header (`preset_diff::assert_exactly_declared`).
//! 2. The preset validates and its session admits it at the 100k Desktop scale.
//! 3. Held-action semantics, phase-stable evaluation schedule (`(n + 1) mod 4
//!    == phase(id)` plus forced newborn first evaluation), exact
//!    `Flow::ControllerCost` accounting (nonzero ONLY on evaluated ticks, zero
//!    on held ticks), `INV-03` energy conservation, and distinct model identity
//!    versus full-evaluation `K = 1`.
//! 4. Bit-exact `WorldDigest` parity across `cpu`, `torch`, and `metal` (where
//!    available) and across worker counts (`w = 1` vs `w = 4`), with zero Metal
//!    deferrals and exact measured evaluation counts.

#![forbid(unsafe_code)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests: an unexpected failure is a test failure"
)]

mod preset_diff;

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use flife_backend_metal::{ControllerChoice, MetalControllers};
use flife_cli::run::configure;
use flife_cli::{RUN_SESSION, VALIDATE_SCHEMA, Validation};
use flife_core::ids::OrganismId;
use flife_core::{Flow, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Organism, Stepped, TickReport, World};
use flife_runtime::SessionConfig;
use preset_diff::assert_exactly_declared;

const PRESET: &str = include_str!("../../../presets/desktop-multirate-4t.toml");
const DESKTOP: &str = include_str!("../../../presets/desktop-world.toml");

/// The preset's declared changes from the Desktop ecology preset, as TOML
/// values: `(key, the Desktop preset's value, this preset's value)`.
const DECLARED: [(&str, &str, &str); 2] = [
    ("name", "\"desktop-world\"", "\"desktop-multirate-4t\""),
    ("controller.evaluation_interval_ticks", "1", "4"),
];

fn compact_multirate_toml(interval: u32) -> String {
    let mut text = PRESET.to_owned();
    for (from, to) in [
        ("width = 512.0\n", "width = 64.0\n"),
        ("height = 512.0\n", "height = 64.0\n"),
        ("grid_width = 512\n", "grid_width = 64\n"),
        ("grid_height = 512\n", "grid_height = 64\n"),
        ("initial_count = 100000\n", "initial_count = 256\n"),
        ("maximum_count = 160000\n", "maximum_count = 1024\n"),
        ("initial_energy = 2.0\n", "initial_energy = 3.0\n"),
        ("count = 128\n", "count = 8\n"),
        ("sigma_world_units = 10.0\n", "sigma_world_units = 4.0\n"),
        (
            "minimum_parent_energy_before_attempt = 3.0\n",
            "minimum_parent_energy_before_attempt = 2.2\n",
        ),
        (
            "maximum_attempt_rate_per_second = 1.0\n",
            "maximum_attempt_rate_per_second = 4.0\n",
        ),
        ("cooldown_seconds = 0.5\n", "cooldown_seconds = 0.25\n"),
        (
            "maximum_logical_genome_bytes = 4294967296\n",
            "maximum_logical_genome_bytes = 268435456\n",
        ),
        (
            "maximum_allocated_genome_bytes = 4831838208\n",
            "maximum_allocated_genome_bytes = 335544320\n",
        ),
        (
            "maximum_total_bytes = 17179869184\n",
            "maximum_total_bytes = 1073741824\n",
        ),
    ] {
        assert!(text.contains(from), "preset missing expected line {from:?}");
        text = text.replacen(from, to, 1);
    }
    if interval != 4 {
        text = text.replacen(
            "evaluation_interval_ticks = 4\n",
            &format!("evaluation_interval_ticks = {interval}\n"),
            1,
        );
    }
    text
}

fn step(world: &mut World) -> TickReport {
    match world.step().expect("step succeeds") {
        Stepped::Published(report) => *report,
        Stepped::Paused(shortfall) => panic!("unexpected capacity pause: {shortfall:?}"),
    }
}

/// The preset is `presets/desktop-world.toml` with exactly its declared
/// changes, and its header declares each one.
#[test]
fn the_preset_is_desktop_world_with_exactly_its_declared_multirate_changes() {
    assert_exactly_declared(DESKTOP, PRESET, &DECLARED);
}

/// The preset validates at the Desktop multi-rate scale (§19.5) and its
/// session admits it.
#[test]
fn the_preset_validates_and_admits_at_desktop_multirate_scale() {
    let config = ValidatedConfig::from_toml(PRESET).expect("valid preset");
    assert_eq!(config.config().controller.evaluation_interval_ticks, 4);
    assert_eq!(config.config().controller.phase_policy, "stable-identity");
    let summary = Validation::of(&config);
    assert_eq!(summary.schema, VALIDATE_SCHEMA);
    assert_eq!(summary.name, "desktop-multirate-4t");
    assert_eq!(summary.model, "field-ecology-v1");
    assert_eq!(summary.seed, 42);
    assert_eq!(summary.grid, [512, 512]);
    assert_eq!(summary.founders, 100_000);
    assert_eq!(summary.maximum_population, 160_000);
    assert_eq!(summary.timestep, [1, 128]);
    assert_eq!(summary.default_parameters, 9988);
    // F16 canonical weights: 2 bytes per parameter.
    assert_eq!(summary.initial_genome_bytes, 100_000 * 9988 * 2);
    assert_eq!(summary.command_durability, "volatile-in-memory");
    assert!(SessionConfig::from_config(RUN_SESSION, &config).is_ok());
}

/// FL-046 core semantics: phase-stable evaluation cadence (`(n + 1) mod 4 ==
/// phase`), forced newborn first evaluation, held-action re-decoding between
/// evaluations, exact `Flow::ControllerCost` billing (positive ONLY on
/// evaluated ticks, strictly 0.0 on held ticks), `INV-03` conservation, and
/// distinct scientific evolution versus full-eval (`K = 1`).
#[test]
fn multirate_held_actions_phase_schedule_and_controller_cost_accounting() {
    let interval = 4_u64;
    let config_k4 = ValidatedConfig::from_toml(&compact_multirate_toml(4)).expect("valid k=4");
    let config_k1 = ValidatedConfig::from_toml(&compact_multirate_toml(1)).expect("valid k=1");
    let profile = NumericalProfile::reference_f32().expect("reference profile");

    let mut world = World::found(&config_k4, Arrangement::FreeList, profile).expect("found k=4");
    let mut full_eval =
        World::found(&config_k1, Arrangement::FreeList, profile).expect("found k=1");

    let c_neural = config_k4
        .config()
        .costs
        .neural_energy_per_parameter_evaluation;
    let watched_id: OrganismId = world
        .population()
        .ids()
        .next()
        .expect("at least one founder");
    world.watch(Some(watched_id));

    let mut total_evaluated = 0_u64;
    let mut total_live_ticks = 0_u64;
    let mut total_births = 0_u64;
    let mut watched_eval_ticks = 0_u64;
    let mut watched_held_ticks = 0_u64;
    let mut max_residual = 0.0_f64;

    for _ in 0..96 {
        let before: BTreeMap<OrganismId, Organism> =
            world.population().iter().map(|row| (row.id, row)).collect();
        let next_tick = world.tick().get() + 1;
        let expected_due = before
            .values()
            .filter(|o| !o.held.is_valid() || next_tick % interval == u64::from(o.phase))
            .count();

        let mut expected_world_ctrl_cost = 0.0_f64;
        for row in before.values() {
            if !row.held.is_valid() || next_tick % interval == u64::from(row.phase) {
                let genome = world
                    .population()
                    .genome(row.id)
                    .expect("live organism has genome");
                let params = u32::try_from(genome.parameters().len()).expect("u32 params");
                expected_world_ctrl_cost += c_neural * f64::from(params);
            }
        }

        let watched_before = before.get(&watched_id).copied();
        let watched_due = watched_before
            .is_some_and(|o| !o.held.is_valid() || next_tick % interval == u64::from(o.phase));

        let report = step(&mut world);
        let _ = step(&mut full_eval);

        assert_eq!(
            report.evaluated, expected_due,
            "tick {next_tick}: evaluated count must match exact phase + newborn forced schedule"
        );
        total_evaluated += u64::try_from(report.evaluated).expect("u64");
        total_live_ticks += u64::try_from(before.len()).expect("u64");
        total_births += u64::try_from(report.births.len()).expect("u64");
        if let Some(norm) = report.ledger.normalized {
            max_residual = max_residual.max(norm);
        }

        // Verify world-level Flow::ControllerCost matches exact sum of c_neural * P over due organisms.
        let actual_ctrl = report.closure.flow(Flow::ControllerCost);
        if report.deaths.is_empty() {
            assert!(
                (actual_ctrl - expected_world_ctrl_cost).abs() <= 1.0e-12,
                "tick {next_tick}: Flow::ControllerCost {actual_ctrl} vs expected {expected_world_ctrl_cost}"
            );
        } else {
            assert!(
                actual_ctrl <= expected_world_ctrl_cost + 1.0e-12,
                "tick {next_tick}: partial-payment Flow::ControllerCost {actual_ctrl} exceeded requested {expected_world_ctrl_cost}"
            );
        }

        // Verify per-organism phase stability, newborn activation, and held-output timestamp.
        let now = world.tick().get();
        for organism in world.population().iter() {
            assert_eq!(u64::from(organism.phase), organism.id.get() % interval);
            if organism.birth_tick.get() == now {
                assert_eq!(
                    organism.held.evaluated_at(),
                    None,
                    "newborn must activate on next tick"
                );
                continue;
            }
            let forced = organism.birth_tick.get() + 1;
            let phase = u64::from(organism.phase);
            let regular = now.checked_sub((now + interval - phase) % interval);
            let expected_eval_tick = regular.filter(|&t| t >= forced).unwrap_or(forced);
            assert_eq!(
                organism.held.evaluated_at().map(|t| t.get()),
                Some(expected_eval_tick),
                "organism {} held output timestamp drifted at tick {now}",
                organism.id
            );
            // Held ticks must preserve raw output bytes while re-decoding against current energy.
            if expected_eval_tick < now
                && let Some(prev) = before.get(&organism.id)
            {
                assert_eq!(
                    organism.held, prev.held,
                    "held raw outputs must not mutate on non-evaluated tick {now}"
                );
            }
        }

        // Verify watched organism's per-tick controller cost billing bit-for-bit
        // on `maint.cost.requested` (exact `(basal + sensor) + c_neural * P` on
        // due ticks vs `basal + sensor` on held ticks; on held ticks `paid.controller`
        // holds at most the ≤1-ULP `(basal + sensor) - basal - sensor` rounding
        // remainder that preserves `ResidualBudget::EXACT`).
        if let Some(snap) = world
            .organism_snapshot(watched_id)
            .expect("watched snapshot")
            && let Some(last_tx) = snap.transactions.last()
            && let Some(maint) = last_tx.maintenance
        {
            let expected_ctrl = c_neural * f64::from(u32::try_from(snap.parameters).expect("u32"));
            let basal_plus_sensor = maint.paid.basal + maint.paid.sensor;
            if watched_due {
                watched_eval_ticks += 1;
                assert_eq!(
                    maint.cost.requested.to_bits(),
                    (basal_plus_sensor + expected_ctrl).to_bits(),
                    "watched organism must request exact (basal + sensor) + c_neural*P on due tick {now}"
                );
                assert!(
                    (maint.paid.controller - expected_ctrl).abs() <= 1.0e-18,
                    "watched organism paid {} vs expected controller cost {expected_ctrl} on due tick {now}",
                    maint.paid.controller
                );
            } else {
                watched_held_ticks += 1;
                assert_eq!(
                    maint.cost.requested.to_bits(),
                    basal_plus_sensor.to_bits(),
                    "watched organism must request exact (basal + sensor) with 0 controller cost on held tick {now}"
                );
                assert!(
                    maint.paid.controller <= 1.0e-18,
                    "watched organism paid unexpected controller cost {} on held tick {now}",
                    maint.paid.controller
                );
            }
        }
    }

    assert!(total_births > 0, "multi-rate ecology must produce births");
    assert!(
        watched_eval_ticks >= 4 && watched_held_ticks >= 12,
        "watched organism must span both evaluated ({watched_eval_ticks}) and held ({watched_held_ticks}) ticks"
    );
    assert!(
        max_residual < 1.0e-5,
        "INV-03 ledger residual {max_residual} exceeded 1e-5"
    );
    // Nominal sweeps = live_ticks / 4, while measured evaluations include forced newborn ticks.
    assert!(
        total_evaluated > total_live_ticks / 4,
        "measured evaluations ({total_evaluated}) must exceed nominal quarter-rate ({}) because of forced newborn evaluations",
        total_live_ticks / 4
    );
    assert_ne!(
        world.digest(),
        full_eval.digest(),
        "K = 4 multi-rate profile must remain a distinct scientific evolution from K = 1 full-eval"
    );
}

/// FL-046 backend parity: the F16 multi-rate profile is bit-exact across CPU
/// reference (`w = 1` and `w = 4`), FrankenTorch CPU (`torch`), and Apple
/// Metal (`metal`, when available) with zero deferrals.
#[test]
fn multirate_desktop_profile_is_bit_exact_across_cpu_torch_and_metal() {
    let config = ValidatedConfig::from_toml(&compact_multirate_toml(4)).expect("valid k=4");
    let profile = NumericalProfile::reference_f32().expect("reference profile");
    let w1 = NonZeroUsize::new(1).expect("1");
    let w4 = NonZeroUsize::new(4).expect("4");

    let run_route = |choice: ControllerChoice, workers: NonZeroUsize| -> (String, u64, u64, u64) {
        let mut world =
            World::found_on(&config, Arrangement::FreeList, profile, workers).expect("found world");
        configure(&mut world, Some(workers), choice, None).expect("configure route");
        let mut evaluated = 0_u64;
        for _ in 0..32 {
            let report = step(&mut world);
            evaluated += u64::try_from(report.evaluated).expect("u64");
        }
        let ctrl = world.controller_report().clone();
        assert_eq!(ctrl.failure, None, "route {choice:?} failed");
        assert_eq!(ctrl.reference_ticks, 0, "route {choice:?} fell back");
        (
            world.digest().to_hex(),
            evaluated,
            ctrl.batched,
            ctrl.deferred,
        )
    };

    let (cpu1_dig, cpu1_eval, _, _) = run_route(ControllerChoice::Cpu, w1);
    let (cpu4_dig, cpu4_eval, _, _) = run_route(ControllerChoice::Cpu, w4);
    assert_eq!(cpu1_dig, cpu4_dig, "w=1 vs w=4 must be bit-exact");
    assert_eq!(cpu1_eval, cpu4_eval);

    let (torch_dig, torch_eval, torch_batched, torch_deferred) =
        run_route(ControllerChoice::Torch, w4);
    assert_eq!(torch_dig, cpu1_dig, "torch vs cpu must be bit-exact");
    assert_eq!(torch_eval, cpu1_eval);
    assert_eq!(torch_batched, cpu1_eval);
    assert_eq!(torch_deferred, 0);

    if MetalControllers::open().is_ok() {
        let (metal_dig, metal_eval, metal_batched, metal_deferred) =
            run_route(ControllerChoice::Metal, w4);
        assert_eq!(metal_dig, cpu1_dig, "metal vs cpu must be bit-exact");
        assert_eq!(metal_eval, cpu1_eval);
        assert_eq!(metal_batched, cpu1_eval);
        assert_eq!(
            metal_deferred, 0,
            "metal must defer 0 multi-rate evaluations"
        );
    }
}
