//! V2-03 tests: Gompertz metabolic senescence ramp (`senescence`).
//!
//! Covers:
//! - Known-answer verification of `senescence_multiplier` and
//!   `MaintenanceCosts::bill_with_basal_multiplier` including extreme-age
//!   overflow clamping to `maximum_multiplier`.
//! - Negative config validation (`scale_seconds <= 0`, `NaN`,
//!   `maximum_multiplier < 1.0`).
//! - Bit-exact baseline parity when disabled.
//! - Exhaustion attribution at `BudgetStage::Maintenance` with exact `INV-03`.
//! - `C.7`, `C.8`, and checkpoint v3 round-trip bit-exactness.
//! - E2E generational-turnover comparison (`senescence` on vs off).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration tests assert directly on invariants and fixtures"
)]

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::clock::TimeStep;
use flife_core::ledger::EnergyBudget;
use flife_core::{FeatureOverride, ValidatedConfig};
use flife_engine::{
    Advance, Arrangement, BudgetStage, DeathCause, MaintenanceCosts, NumericalProfile, Stepped,
    WORLD_CHECKPOINT_CHUNKS, World, senescence_multiplier,
};

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
        ("world", "timestep_seconds", "0.03125"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "128"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "200.0"),
        ("sources", "sigma_world_units", "3.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.5"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.0",
        ),
        ("reproduction", "maximum_attempt_rate_per_second", "4.0"),
        ("reproduction", "cooldown_seconds", "0.25"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ]
}

#[test]
fn senescence_exact_formula_and_overflow_clamp() {
    let tau = 30.0_f64;
    let max_m = 64.0_f64;
    let step = TimeStep::new(1, 32).expect("step");
    let costs = MaintenanceCosts::new(0.8, 0.1, 1.0, 1.0, 1e-5).expect("costs");

    for (age, want_m) in [
        (0.0, 1.0),
        (0.5 * tau, 1.25),
        (tau, 2.0),
        (2.0 * tau, 5.0),
        (1e6 * tau, max_m),
        (1e200 * tau, max_m),
    ] {
        let m = senescence_multiplier(age, tau, max_m).expect("valid mult");
        assert!(m.is_finite(), "age={age} produced nonfinite multiplier");
        assert!(
            (m - want_m).abs() <= 1e-12,
            "age={age}: got m={m}, want {want_m}"
        );
        let bill = costs
            .bill_with_basal_multiplier(step, 1.0, 1000, false, m)
            .expect("bill");
        let want_basal = (step.seconds_per_tick() * 0.8) * m;
        assert_eq!(bill.basal.to_bits(), want_basal.to_bits());

        let mut budget = EnergyBudget::new(10.0).expect("budget");
        let settled = bill.settle(&mut budget).expect("settle");
        assert_eq!(settled.paid.basal.to_bits(), want_basal.to_bits());
    }

    // Young organism (age < 3 s at tau = 30 s) pays < 1% extra basal maintenance.
    let young = senescence_multiplier(2.9, tau, max_m).expect("young");
    assert!(young < 1.01, "young m_age={young} exceeded 1.01");

    // Baseline multiplier 1.0 matches `bill()` bit-for-bit.
    let baseline = costs.bill(step, 1.5, 4096, true).expect("baseline");
    let mult_one = costs
        .bill_with_basal_multiplier(step, 1.5, 4096, true, 1.0)
        .expect("mult 1");
    assert_eq!(baseline.basal.to_bits(), mult_one.basal.to_bits());
    assert_eq!(baseline.sensor.to_bits(), mult_one.sensor.to_bits());
    assert_eq!(baseline.controller.to_bits(), mult_one.controller.to_bits());
}

#[test]
fn senescence_invalid_config_rejected() {
    assert!(senescence_multiplier(1.0, 0.0, 64.0).is_err());
    assert!(senescence_multiplier(1.0, -5.0, 64.0).is_err());
    assert!(senescence_multiplier(1.0, f64::NAN, 64.0).is_err());
    assert!(senescence_multiplier(1.0, 30.0, 0.99).is_err());
    assert!(senescence_multiplier(1.0, 30.0, f64::NAN).is_err());
    assert!(senescence_multiplier(-1.0, 30.0, 64.0).is_err());

    for bad_tail in [
        "[v2.senescence]\nenabled = true\nscale_seconds = 0.0\n",
        "[v2.senescence]\nenabled = true\nscale_seconds = -1.0\n",
        "[v2.senescence]\nenabled = true\nscale_seconds = nan\n",
        "[v2.senescence]\nenabled = true\nmaximum_multiplier = 0.5\n",
        "[v2.senescence]\nenabled = true\nmaximum_multiplier = nan\n",
    ] {
        let toml = ecology_toml("field-ecology-v2", small_overrides(), bad_tail);
        assert!(
            ValidatedConfig::from_toml(&toml).is_err(),
            "invalid senescence config must fail validation:\n{bad_tail}"
        );
    }
}

#[test]
fn senescence_disabled_matches_baseline_bit_exact() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let v1_cfg =
        ValidatedConfig::from_toml(&ecology_toml("field-ecology-v1", small_overrides(), ""))
            .expect("v1");
    let v2_cfg = ValidatedConfig::from_toml_with_overrides(
        &ecology_toml("field-ecology-v2", small_overrides(), ""),
        &[FeatureOverride::V1Compat],
    )
    .expect("v2 disabled");
    let mut w1 = World::found(&v1_cfg, Arrangement::Compact, profile).expect("w1");
    let mut w2 = World::found(&v2_cfg, Arrangement::Compact, profile).expect("w2");
    for _ in 0..48 {
        assert!(matches!(w1.step().expect("w1"), Stepped::Published(_)));
        assert!(matches!(w2.step().expect("w2"), Stepped::Published(_)));
    }
    assert_eq!(w1.digest(), w2.digest());
}

#[test]
fn senescence_exhaustion_reports_maintenance_stage() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    // Short senescence scale (1.0 s) and low source power so aged founders
    // exhaust cleanly on the Maintenance stage.
    let toml = ecology_toml(
        "field-ecology-v2",
        &[
            ("world", "width", "16.0"),
            ("world", "height", "16.0"),
            ("world", "grid_width", "32"),
            ("world", "grid_height", "32"),
            ("world", "timestep_seconds", "0.03125"),
            ("population", "initial_count", "16"),
            ("population", "maximum_count", "64"),
            ("population", "initial_energy", "2.0"),
            ("sources", "count", "1"),
            ("sources", "power_per_source", "0.0"),
            ("sources", "sigma_world_units", "2.0"),
            ("sources", "path_direction_interval_seconds", "2.0"),
            ("costs", "basal_energy_per_second", "0.5"),
            ("execution", "command_durability", "\"volatile-in-memory\""),
        ],
        r#"
[v2]
profile = "minimal"

[v2.senescence]
enabled = true
scale_seconds = 1.0
maximum_multiplier = 64.0
"#,
    );
    let cfg = ValidatedConfig::from_toml(&toml).expect("valid senescence config");
    let mut world = World::found(&cfg, Arrangement::Compact, profile).expect("world");
    let mut maintenance_deaths = 0_usize;
    let mut max_residual = 0.0_f64;
    for _ in 0..128 {
        let Stepped::Published(report) = world.step().expect("step") else {
            panic!("unexpected pause");
        };
        max_residual = max_residual.max(report.closure.residual().abs());
        for death in &report.deaths {
            if death.cause == DeathCause::Exhausted(BudgetStage::Maintenance) {
                maintenance_deaths += 1;
            }
        }
        if world.population().is_empty() {
            break;
        }
    }
    assert!(
        maintenance_deaths > 0,
        "senescence exhaustion must report BudgetStage::Maintenance"
    );
    assert!(
        max_residual < 1e-5,
        "INV-03 residual {max_residual:e} exceeded 1e-5"
    );
}

#[test]
fn senescence_c8_cancel_and_checkpoint_roundtrip() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[v2]
profile = "minimal"

[v2.senescence]
enabled = true
scale_seconds = 4.0
maximum_multiplier = 64.0
"#,
    );
    let cfg = ValidatedConfig::from_toml(&toml).expect("cfg");
    let mut compact = World::found(&cfg, Arrangement::Compact, profile).expect("compact");
    let mut freelist = World::found(&cfg, Arrangement::FreeList, profile).expect("freelist");

    for tick in 0..64 {
        if tick == 32 {
            let before = compact.digest();
            if let Advance::Ready(cand) = compact.advance().expect("advance") {
                cand.cancel();
            }
            assert_eq!(compact.digest(), before, "C.8 cancel preserves state");
        }
        assert!(matches!(compact.step().expect("c"), Stepped::Published(_)));
        assert!(matches!(freelist.step().expect("f"), Stepped::Published(_)));
    }
    assert_eq!(
        compact.digest(),
        freelist.digest(),
        "C.7 arrangement parity"
    );

    let mut writer = CheckpointWriter::new(&cfg.config().model);
    compact.write_checkpoint(&mut writer).expect("write");
    let bytes = writer.finish().expect("finish");
    let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("open");
    let mut restored = World::read_checkpoint(&reader, profile).expect("restore");
    assert_eq!(restored.digest(), compact.digest());

    for _ in 0..64 {
        assert!(matches!(compact.step().expect("c2"), Stepped::Published(_)));
        assert!(matches!(
            restored.step().expect("r2"),
            Stepped::Published(_)
        ));
    }
    assert_eq!(
        restored.digest(),
        compact.digest(),
        "checkpoint round-trip must resume bit-identically under senescence"
    );
}

#[test]
fn e2e_senescence_increases_generational_turnover() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    // Compact crowded world hitting population cap where immortal adults
    // otherwise freeze slot turnover; senescence (tau_sen = 4.0 s over 32 s
    // = 1,024 ticks) turns over aging adults cleanly.
    let overrides: &[(&str, &str, &str)] = &[
        ("world", "width", "16.0"),
        ("world", "height", "16.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("world", "timestep_seconds", "0.03125"),
        ("population", "initial_count", "64"),
        ("population", "maximum_count", "96"),
        ("population", "initial_energy", "3.0"),
        (
            "population",
            "capacity_policy",
            "\"refuse-excess-births-v1\"",
        ),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "450.0"),
        ("sources", "sigma_world_units", "4.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "10.0"),
        ("costs", "basal_energy_per_second", "0.5"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.0",
        ),
        ("reproduction", "maximum_attempt_rate_per_second", "4.0"),
        ("reproduction", "cooldown_seconds", "0.25"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ];
    let on_toml = ecology_toml(
        "field-ecology-v2",
        overrides,
        r#"
[v2]
profile = "minimal"

[v2.senescence]
enabled = true
scale_seconds = 3.0
maximum_multiplier = 64.0
"#,
    );
    let off_toml = ecology_toml(
        "field-ecology-v2",
        overrides,
        r#"
[v2]
profile = "minimal"
"#,
    );
    let on_cfg = ValidatedConfig::from_toml(&on_toml).expect("on");
    let off_cfg = ValidatedConfig::from_toml(&off_toml).expect("off");

    let run = |cfg: &ValidatedConfig| {
        let mut world = World::found(cfg, Arrangement::Compact, profile).expect("world");
        let mut births_total = 0_usize;
        let mut max_residual = 0.0_f64;
        for _ in 0..1024 {
            let Stepped::Published(report) = world.step().expect("step") else {
                panic!("unexpected pause");
            };
            births_total += report.births.len();
            max_residual = max_residual.max(report.closure.residual().abs());
        }
        let tick = world.tick().get();
        let live = world.population().len();
        let mean_gen = if live > 0 {
            world
                .population()
                .iter()
                .map(|o| f64::from(o.generation.get()))
                .sum::<f64>()
                / live as f64
        } else {
            0.0
        };
        let max_age_ticks = world
            .population()
            .iter()
            .map(|o| tick - o.birth_tick.get())
            .max()
            .unwrap_or(0);
        (live, mean_gen, max_age_ticks, births_total, max_residual)
    };

    let (live_on, gen_on, age_on, births_on, res_on) = run(&on_cfg);
    let (live_off, gen_off, age_off, births_off, res_off) = run(&off_cfg);
    eprintln!(
        "senescence E2E (1024 ticks): ON(live={live_on}, mean_gen={gen_on:.2}, max_age={age_on}, births={births_on}, res={res_on:e}) \
         vs OFF(live={live_off}, mean_gen={gen_off:.2}, max_age={age_off}, births={births_off}, res={res_off:e})"
    );
    assert!(res_on < 1e-5, "INV-03 residual {res_on:e} exceeded 1e-5");
    assert!(live_on > 0, "population must survive under senescence");
    assert!(
        gen_on > gen_off,
        "senescence must increase mean generation ({gen_on:.2} vs {gen_off:.2})"
    );
    assert!(
        age_on < age_off,
        "senescence must bound maximum organism age ({age_on} vs {age_off})"
    );
}
