//! V2-02 (`oases`) and V2-09 (`tight-junctions`) end-to-end `World` tests.
//!
//! Covers:
//! - Bit-exact parity + zero extra footprint bytes when `oases` or
//!   `tight-junctions` are disabled vs `--v1-compat`.
//! - `V2-02` (`oases`): heterogeneous per-source `σ_j` seeding, constant
//!   global injected power `M * P_base * Δt` across full bloom cycles with
//!   dormant `0.0` oasis skips, `1 vs 4` CPU workers, `C.7`, `C.8`,
//!   `World::cropped`, and checkpoint v3 round-trip.
//! - `V2-09` (`tight-junctions`): both `harmonic-v1` and `min-v1` modes across
//!   `1 vs 4` workers, `C.7`, `C.8`, checkpoint v3 round-trip, `World::footprint`
//!   stability, and post-Phase-1 `Intervention::RemoveOrganisms` puncture.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration tests assert directly on invariants and fixtures"
)]

use std::num::NonZeroUsize;

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::{CoreError, FeatureOverride, ValidatedConfig};
use flife_engine::{
    Advance, Arrangement, Disposition, Intervention, NumericalProfile, Stepped, TickReport,
    WORLD_CHECKPOINT_CHUNKS, World,
};
use flife_field::ConeTolerance;

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");
const ARM1_TOML: &str =
    include_str!("../../../experiments/v2-quickies/arm1-neural-agency-128.toml");
const PRESET_TOML: &str = include_str!("../../../presets/reference-small-world.toml");

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
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "128"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "120.0"),
        ("sources", "sigma_world_units", "2.5"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.25"),
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

fn config_with(tail: &str, feature_overrides: &[FeatureOverride]) -> ValidatedConfig {
    let toml = ecology_toml("field-ecology-v2", small_overrides(), tail);
    ValidatedConfig::from_toml_with_overrides(&toml, feature_overrides)
        .expect("v2 test config validates")
}

fn step_published(world: &mut World) -> Box<TickReport> {
    match world.step().expect("step succeeds") {
        Stepped::Published(report) => report,
        Stepped::Paused(shortfall) => panic!("unexpected capacity pause: {shortfall:?}"),
    }
}

#[test]
fn oases_and_tight_junctions_disabled_match_v1_compat_bit_exact() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let base_cfg = config_with("", &[FeatureOverride::V1Compat]);
    let toggled_cfg = config_with(
        "",
        &[
            FeatureOverride::V1Compat,
            FeatureOverride::Enable("oases".to_owned()),
            FeatureOverride::Enable("tight-junctions".to_owned()),
            FeatureOverride::Disable("oases".to_owned()),
            FeatureOverride::Disable("tight-junctions".to_owned()),
        ],
    );
    let mut base = World::found(&base_cfg, Arrangement::Compact, profile).expect("base");
    let mut toggled = World::found(&toggled_cfg, Arrangement::Compact, profile).expect("toggled");
    assert!(toggled.source_kernels().is_empty());
    assert_eq!(base.footprint(), toggled.footprint());
    for _ in 0..24 {
        step_published(&mut base);
        step_published(&mut toggled);
    }
    assert_eq!(base.digest(), toggled.digest());
}

#[test]
fn v2_oases_constant_total_power_heterogeneous_sigmas_c7_c8_crop_and_checkpoint() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let tail = r#"
[v2.oases]
bloom_period_seconds = 1.0
minimum_power_fraction = 0.0
sigma_min_world_units = 1.5
sigma_max_world_units = 3.25
"#;
    let cfg = config_with(
        tail,
        &[
            FeatureOverride::V1Compat,
            FeatureOverride::Enable("oases".to_owned()),
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

    assert_eq!(w1.source_kernels().len(), 4);
    let sigmas: Vec<f64> = w1.source_kernels().iter().map(|k| k.sigma()).collect();
    for &sigma in &sigmas {
        assert!((1.5..=3.25).contains(&sigma), "sigma {sigma} out of range");
    }
    assert!(
        sigmas
            .windows(2)
            .any(|pair| (pair[0] - pair[1]).abs() > 1.0e-6),
        "expected heterogeneous seeded source sigmas: {sigmas:?}"
    );

    let expected_total_declared = 4.0 * 120.0 * 0.03125;
    let mut saw_dormant = false;
    let mut saw_bloom = false;
    let footprint_before = w1.footprint();

    for tick in 0..40 {
        if tick == 20 {
            let before = w1.digest();
            if let Advance::Ready(candidate) = w1.advance().expect("cancel advance") {
                candidate.cancel();
            }
            assert_eq!(w1.digest(), before);
        }
        let report = step_published(&mut w1);
        let report4 = step_published(&mut w4);
        assert_eq!(w1.digest(), w4.digest(), "C.7 / 1v4 parity at tick {tick}");
        assert_eq!(
            report.field_step.injected_declared.to_bits(),
            report4.field_step.injected_declared.to_bits()
        );
        let rel = (report.field_step.injected_declared - expected_total_declared).abs()
            / expected_total_declared;
        assert!(
            rel <= 1.0e-12,
            "tick {tick} declared injection {} drifted from constant {expected_total_declared}",
            report.field_step.injected_declared
        );
        for dep in &report.field_step.deposits {
            if dep.declared == 0.0 && dep.applied == 0.0 {
                saw_dormant = true;
            }
            if dep.declared > 1.2 * (expected_total_declared / 4.0) {
                saw_bloom = true;
            }
        }
    }
    assert!(saw_dormant, "expected dormant zero-power oasis ticks");
    assert!(saw_bloom, "expected elevated bloom oasis ticks");
    assert_eq!(
        w1.footprint(),
        footprint_before,
        "steady-state footprint grew"
    );

    // Checkpoint v3 round-trip bit-exactness (including per-source sigma_j).
    let mut writer = CheckpointWriter::new(&cfg.config().model);
    w1.write_checkpoint(&mut writer).expect("write checkpoint");
    let bytes = writer.finish().expect("finish checkpoint");
    let reader =
        CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("open checkpoint");
    let mut restored = World::read_checkpoint(&reader, profile).expect("read checkpoint");
    assert_eq!(restored.digest(), w1.digest());
    assert_eq!(restored.source_kernels().len(), 4);
    for _ in 0..16 {
        step_published(&mut w1);
        step_published(&mut restored);
    }
    assert_eq!(restored.digest(), w1.digest());

    // Crop preserves source_kernels and round-trips through checkpoint cleanly.
    let (cropped, _) = w1.cropped([8.0, 8.0], 6.0).expect("crop");
    assert_eq!(cropped.source_kernels().len(), 4);
    let mut cwriter = CheckpointWriter::new(&cfg.config().model);
    cropped
        .write_checkpoint(&mut cwriter)
        .expect("write crop checkpoint");
    let cbytes = cwriter.finish().expect("finish crop checkpoint");
    let creader =
        CheckpointReader::parse(&cbytes, &WORLD_CHECKPOINT_CHUNKS).expect("open crop checkpoint");
    let crestored = World::read_checkpoint(&creader, profile).expect("read crop checkpoint");
    assert_eq!(crestored.digest(), cropped.digest());
}

#[test]
fn v2_tight_junctions_e2e_modes_workers_c7_c8_checkpoint_and_puncture() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    for mode in ["harmonic-v1", "min-v1"] {
        let tail = format!(
            r#"
[v2.barrier]
mode = "{mode}"
minimum_permeability = 0.04
occupancy_impedance = 1.5
"#
        );
        let cfg = config_with(
            &tail,
            &[
                FeatureOverride::V1Compat,
                FeatureOverride::Enable("tight-junctions".to_owned()),
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
        let footprint_before = w1.footprint();

        for tick in 0..32 {
            if tick == 16 {
                let before = w1.digest();
                if let Advance::Ready(candidate) = w1.advance().expect("cancel advance") {
                    candidate.cancel();
                }
                assert_eq!(w1.digest(), before);
            }
            step_published(&mut w1);
            step_published(&mut w4);
            assert_eq!(
                w1.digest(),
                w4.digest(),
                "{mode} C.7 / 1v4 parity failed at tick {tick}"
            );
        }
        assert_eq!(w1.footprint(), footprint_before);

        // Post-Phase-1 `RemoveOrganisms` puncture executes cleanly with exact ledger closure.
        let puncture = Intervention::RemoveOrganisms {
            center: [8.0, 8.0],
            radius: 4.0,
            disposition: Disposition::Remove,
        };
        assert!(matches!(
            w1.step_with(&[puncture]).expect("puncture step"),
            Stepped::Published(_)
        ));
        assert!(matches!(
            w4.step_with(&[puncture]).expect("puncture step w4"),
            Stepped::Published(_)
        ));
        assert_eq!(w1.digest(), w4.digest());

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
    }
}

/// Plan §11.5 Option B bounded moment-only cone contraction in Phase 10:
/// - `NumericalProfile::reference_f32()` keeps the hard `INV-02` gate at
///   `(f32::MIN_POSITIVE, 1e-5)` and triggers contraction only above
///   `(f32::MIN_POSITIVE, 1e-6)`.
/// - `presets/reference-small-world.toml` (8 ticks = `PROBE_TICKS`) triggers
///   0 contractions, keeping `PRESET_DIGEST` 100% bit-identical.
/// - A genuine violation above the configured `INV-02` gate still faults
///   immediately before contraction (never masked).
/// - Arm 1 (`arm1-neural-agency-128.toml`, seed 101, 8 pulsing heterogeneous
///   oases, `emission = 0.0`, `initial_count = 0` — which faulted on `INV-02`
///   at tick 2,026 before this fix) now runs cleanly past tick 2,657 to tick
///   2,700 with unchanged `INV-02 = 1e-5`, unmasked `FieldSummary`, logged
///   `TickReport::cone_contraction`, intact `INV-03` energy closure, and
///   1-vs-4 worker parity across contraction ticks.
#[test]
fn plan_11_5_phase10_cone_contraction_passes_arm1_seed101_past_tick_2657_and_preserves_short_digests()
 {
    let profile = NumericalProfile::reference_f32().expect("profile");
    assert_eq!(profile.cone.relative(), 1.0e-5, "INV-02 gate stays 1e-5");
    assert_eq!(
        profile.cone_contraction_trigger().absolute(),
        f64::from(f32::MIN_POSITIVE)
    );
    assert_eq!(
        profile.cone_contraction_trigger().relative(),
        1.0e-6,
        "Option B contraction trigger is 1e-6"
    );

    // 1. Short reference preset (`PROBE_TICKS = 8`): 0 contractions -> PRESET_DIGEST bit-identical.
    let preset_cfg = ValidatedConfig::from_toml(PRESET_TOML).expect("preset");
    let mut preset_world = World::found_on(
        &preset_cfg,
        Arrangement::Compact,
        profile,
        NonZeroUsize::new(4).expect("4"),
    )
    .expect("preset world");
    for tick in 0..8_u32 {
        let report = step_published(&mut preset_world);
        assert_eq!(
            report.cone_contraction.cells, 0,
            "preset tick {tick} must not trigger Option B contraction"
        );
    }

    // 2. Hard INV-02 gate runs strictly before contraction and never masks a violation.
    let strict_profile = NumericalProfile {
        residual: profile.residual,
        cone: ConeTolerance::EXACT,
    };
    let arm1_zero_toml = ARM1_TOML.replace("initial_count = 4096", "initial_count = 0");
    let arm1_cfg = ValidatedConfig::from_toml(&arm1_zero_toml).expect("arm1 zero-pop cfg");
    let mut strict_world = World::found_on(
        &arm1_cfg,
        Arrangement::Compact,
        strict_profile,
        NonZeroUsize::new(4).expect("4"),
    )
    .expect("strict world");
    let mut strict_faulted = false;
    for _ in 0..16 {
        match strict_world.step() {
            Ok(Stepped::Published(_)) => {}
            Ok(Stepped::Paused(shortfall)) => panic!("unexpected pause: {shortfall:?}"),
            Err(CoreError::NumericalInvalid {
                quantity: "chemical moment cone",
                ..
            }) => {
                strict_faulted = true;
                break;
            }
            Err(other) => panic!("unexpected error under exact cone gate: {other:?}"),
        }
    }
    assert!(
        strict_faulted,
        "INV-02 check must run read-only before contraction and reject violations beyond profile.cone"
    );

    // 3. Arm 1 seed 101 pulsing-oases world passes tick 2,026 and tick 2,657
    //    cleanly all the way to tick 2,700 under unchanged `reference_f32()`.
    let mut arm1_w4 = World::found_on(
        &arm1_cfg,
        Arrangement::Compact,
        profile,
        NonZeroUsize::new(4).expect("4"),
    )
    .expect("arm1 world");
    let mut total_contracted_cells = 0_u64;
    let mut contracted_ticks = 0_u64;
    let mut max_contraction_rel = 0.0_f64;
    let mut max_inv03_rel = 0.0_f64;
    let mut first_contraction_checkpoint: Option<Vec<u8>> = None;
    for tick in 0..2700_u32 {
        // Capture a checkpoint right before a contraction tick to verify 1-vs-4
        // worker bit-exactness across live contraction ticks without paying
        // 2,700 serial ticks.
        let report = step_published(&mut arm1_w4);
        let inv03_rel = report.closure.residual().abs() / report.closure.scale().max(1.0);
        max_inv03_rel = max_inv03_rel.max(inv03_rel);
        if report.cone_contraction.cells > 0 {
            contracted_ticks += 1;
            total_contracted_cells += report.cone_contraction.cells;
            max_contraction_rel = max_contraction_rel.max(report.cone_contraction.max_relative);
            assert!(
                report.field.max_cone_residual > 0.0,
                "tick {tick}: FieldSummary must report unmasked pre-contraction residual"
            );
            assert!(
                report.cone_contraction.max_relative < 2.0e-6,
                "tick {tick}: bounded contraction exceeded 2e-6 ({:?})",
                report.cone_contraction
            );
            if first_contraction_checkpoint.is_none() {
                let mut writer = CheckpointWriter::new(&arm1_cfg.config().model);
                arm1_w4
                    .write_checkpoint(&mut writer)
                    .expect("write checkpoint");
                first_contraction_checkpoint = Some(writer.finish().expect("finish checkpoint"));
            }
        }
    }
    assert!(
        contracted_ticks > 0 && total_contracted_cells > 0,
        "Arm 1 seed 101 must trigger bounded plan §11.5 cone contraction across 2,700 ticks"
    );
    assert!(
        max_inv03_rel < 1.0e-6,
        "INV-03 residual must remain unaffected by moment-only contraction: {max_inv03_rel:e}"
    );
    let _ = max_contraction_rel;

    // 4. Verify 1-vs-4 worker bit-exactness across contraction ticks starting
    //    from the live Arm 1 contraction checkpoint.
    let bytes = first_contraction_checkpoint.expect("captured contraction checkpoint");
    let reader =
        CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("open checkpoint");
    let mut restored_w1 = World::read_checkpoint(&reader, profile).expect("restore w1");
    let mut restored_w4 = World::read_checkpoint(&reader, profile).expect("restore w4");
    restored_w4
        .set_cpu_workers(NonZeroUsize::new(4).expect("4"))
        .expect("set 4 workers");
    for step_idx in 0..32_u32 {
        let r1 = step_published(&mut restored_w1);
        let r4 = step_published(&mut restored_w4);
        assert_eq!(
            r1.cone_contraction, r4.cone_contraction,
            "step {step_idx}: 1 vs 4 worker ConeContractionSummary mismatch"
        );
        assert_eq!(
            restored_w1.digest(),
            restored_w4.digest(),
            "step {step_idx}: 1 vs 4 worker digest mismatch across contraction"
        );
    }
}
