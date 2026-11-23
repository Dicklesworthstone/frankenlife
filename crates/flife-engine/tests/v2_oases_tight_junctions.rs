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
use flife_core::{FeatureOverride, ValidatedConfig};
use flife_engine::{
    Advance, Arrangement, Disposition, Intervention, NumericalProfile, Stepped, TickReport,
    WORLD_CHECKPOINT_CHUNKS, World,
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
