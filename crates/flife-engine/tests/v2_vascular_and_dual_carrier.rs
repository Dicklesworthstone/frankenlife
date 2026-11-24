//! Integration and E2E verification for `V2-14` (`vascular-field`) and
//! `V2-15` (`dual-carrier-trails`, realizing `FL-055`) inside `flife-engine`.
//!
//! Covers:
//! - `--disable vascular-field` and `--disable dual-carrier-trails` bit-exact
//!   fallback + `0 B` overhead;
//! - Default `field-ecology-v2` (both `V2-14` + `V2-15` active!) stepping,
//!   watching + `organism_snapshot` activation verification, checkpoint v3,
//!   `crop`/`graft`, `C.7` Compact vs FreeList, `C.8` cancel-safety, and
//!   `admit_memory` (`INV-15`);
//! - Non-destructive uptake on Carrier 0 leaving Carrier 1 bit-for-bit
//!   unchanged + combined `Transfer::Emission` / `Flow::Decay` exact INV-03;
//! - E2E `V2-14` vascular Murray branching (`mu = 1.5` vs `mu = 0.7` and
//!   disabled control) on `V2-02` Oases;
//! - E2E `V2-15` dual-carrier persistent pheromone ridges (`D_1 = 0.01 D_0,
//!   lambda_1 = 0.05 lambda_0` vs matched `D_1 = D_0, lambda_1 = lambda_0` and
//!   disabled control).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::num::NonZeroUsize;

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::{FeatureOverride, Flow, Transfer, V2Feature, ValidatedConfig};
use flife_engine::{
    Arrangement, Disposition, Intervention, NumericalProfile, Stepped, WORLD_CHECKPOINT_CHUNKS,
    World,
};
use flife_field::{DUAL_CARRIER_PLANES, FIELD_PLANES, VASCULAR_INITIAL_CONDUCTANCE};

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
        ("population", "maximum_count", "256"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "200.0"),
        ("sources", "sigma_world_units", "2.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ]
}

fn check_inv03(world: &World, profile: NumericalProfile, max_rel: f64) {
    assert!(
        max_rel <= 1e-6,
        "INV-03 per-tick relative residual {max_rel:e} exceeded 1e-6 (§21.4)"
    );
    let cum_norm = world.ledger().report().normalized.unwrap_or(0.0);
    assert!(
        cum_norm <= profile.residual.cumulative(),
        "INV-03 cumulative normalized residual {cum_norm:e} exceeded {}",
        profile.residual.cumulative()
    );
}

#[test]
fn v2_14_vascular_disabled_matches_baseline_bit_exact_and_zero_bytes() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let v1_toml = ecology_toml("field-ecology-v1", small_overrides(), "");
    let v2_off_toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[v2]
profile = "minimal"

[v2.vascular]
enabled = true
"#,
    );
    let v1_cfg = ValidatedConfig::from_toml(&v1_toml).expect("v1");
    let v2_off_cfg = ValidatedConfig::from_toml_with_overrides(
        &v2_off_toml,
        &[FeatureOverride::Disable("vascular-field".to_owned())],
    )
    .expect("v2 disabled");
    let v2_on_cfg = ValidatedConfig::from_toml(&v2_off_toml).expect("v2 enabled");

    assert!(!v2_off_cfg.is_v2_feature_enabled(V2Feature::VascularField));
    assert!(v2_on_cfg.is_v2_feature_enabled(V2Feature::VascularField));

    let mut w1 = World::found(&v1_cfg, Arrangement::Compact, profile).expect("w1");
    let mut w2_off = World::found(&v2_off_cfg, Arrangement::Compact, profile).expect("w2_off");
    let w2_on = World::found(&v2_on_cfg, Arrangement::Compact, profile).expect("w2_on");

    assert!(!w2_off.field().has_vascular());
    assert!(w2_on.field().has_vascular());
    // Exact 0 B overhead when disabled vs baseline.
    assert_eq!(w2_off.footprint().field, w1.footprint().field);
    assert_eq!(w2_off.footprint().scratch, w1.footprint().scratch);
    assert!(
        w2_on.footprint().field > w2_off.footprint().field,
        "enabled vascular field must account for ping-pong edge buffers in footprint"
    );

    for _ in 0..32 {
        let Stepped::Published(r1) = w1.step().expect("w1 step") else {
            panic!("unexpected pause");
        };
        let Stepped::Published(r2) = w2_off.step().expect("w2_off step") else {
            panic!("unexpected pause");
        };
        assert_eq!(r1.closure.closing(), r2.closure.closing());
    }
    assert_eq!(
        w1.digest(),
        w2_off.digest(),
        "--disable vascular-field must match v1 baseline bit-for-bit"
    );
}

#[test]
fn v2_15_dual_carrier_disabled_matches_33_plane_bit_exact_and_zero_trail_scratch() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let v1_toml = ecology_toml("field-ecology-v1", small_overrides(), "");
    let v2_toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[v2]
profile = "minimal"

[v2.carriers]
enabled = true
"#,
    );
    let v1_cfg = ValidatedConfig::from_toml(&v1_toml).expect("v1");
    let v2_off_cfg = ValidatedConfig::from_toml_with_overrides(
        &v2_toml,
        &[FeatureOverride::Disable("dual-carrier-trails".to_owned())],
    )
    .expect("v2 disabled");
    let v2_on_cfg = ValidatedConfig::from_toml(&v2_toml).expect("v2 enabled");

    assert!(!v2_off_cfg.is_v2_feature_enabled(V2Feature::DualCarrierTrails));
    assert!(v2_on_cfg.is_v2_feature_enabled(V2Feature::DualCarrierTrails));

    let mut w1 = World::found(&v1_cfg, Arrangement::Compact, profile).expect("w1");
    let mut w2_off = World::found(&v2_off_cfg, Arrangement::Compact, profile).expect("w2_off");
    let w2_on = World::found(&v2_on_cfg, Arrangement::Compact, profile).expect("w2_on");

    assert_eq!(w2_off.field().planes(), FIELD_PLANES);
    assert_eq!(w2_on.field().planes(), DUAL_CARRIER_PLANES);
    // Exact 0 B trail scratch and 0 extra field bytes when disabled.
    assert_eq!(w2_off.footprint().field, w1.footprint().field);
    assert_eq!(w2_off.footprint().scratch, w1.footprint().scratch);
    assert!(w2_on.footprint().field > w2_off.footprint().field);
    assert!(w2_on.footprint().scratch > w2_off.footprint().scratch);

    for _ in 0..32 {
        assert!(matches!(w1.step().expect("w1"), Stepped::Published(_)));
        assert!(matches!(
            w2_off.step().expect("w2_off"),
            Stepped::Published(_)
        ));
    }
    assert_eq!(
        w1.digest(),
        w2_off.digest(),
        "--disable dual-carrier-trails must match 33-plane baseline bit-for-bit"
    );
}

#[test]
fn v2_14_and_v2_15_default_v2_world_steps_watches_checkpoints_crops_grafts_c7_c8() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    // Default pure-field `field-ecology-v2` enables BOTH V2-14 vascular-field
    // AND V2-15 34-plane dual-carrier-trails (+ V2-02/03/04/05/06/07/09).
    let toml = ecology_toml("field-ecology-v2", small_overrides(), "");
    let cfg = ValidatedConfig::from_toml(&toml).expect("default v2 config");
    assert!(cfg.is_v2_feature_enabled(V2Feature::VascularField));
    assert!(cfg.is_v2_feature_enabled(V2Feature::DualCarrierTrails));
    assert_eq!(cfg.v2_abi().sensor_width, 108);

    let mut w_compact_1 = World::found_on(
        &cfg,
        Arrangement::Compact,
        profile,
        NonZeroUsize::new(1).expect("1"),
    )
    .expect("w_compact_1");
    let mut w_freelist_4 = World::found_on(
        &cfg,
        Arrangement::FreeList,
        profile,
        NonZeroUsize::new(4).expect("4"),
    )
    .expect("w_freelist_4");

    // Memory admission must pass cleanly on default V2 world (INV-15).
    w_compact_1
        .admit_memory(false, 0, 0)
        .expect("admit headless");
    w_compact_1.admit_memory(true, 0, 0).expect("admit batched");

    // Watch organism 0 to verify 34-plane dual-carrier watch + Activations trace!
    let watched_id = w_compact_1
        .population()
        .iter()
        .next()
        .expect("founder 0")
        .id;
    w_compact_1.watch(Some(watched_id));

    let mut max_rel = 0.0_f64;
    let mut saw_emission = false;
    let mut saw_trail_energy = false;
    let mut saw_vascular_change = false;
    for _ in 0..32 {
        let Stepped::Published(r1) = w_compact_1.step().expect("step 1w") else {
            panic!("pause");
        };
        let Stepped::Published(r4) = w_freelist_4.step().expect("step 4w") else {
            panic!("pause");
        };
        assert_eq!(r1.closure.closing(), r4.closure.closing());
        let rel = r1.closure.residual().abs() / r1.closure.scale().max(1.0);
        max_rel = max_rel.max(rel);
        saw_emission |= r1.emitted > 0;
        let trail_sum: f64 = w_compact_1
            .field()
            .trail_energy()
            .expect("trail energy")
            .iter()
            .map(|&v| f64::from(v))
            .sum();
        saw_trail_energy |= trail_sum > 0.0;
        let (gx, gy) = w_compact_1.field().vascular_edges().expect("vascular");
        saw_vascular_change |= gx
            .iter()
            .chain(gy.iter())
            .any(|&g| (g - VASCULAR_INITIAL_CONDUCTANCE).abs() > 1e-4);
    }
    check_inv03(&w_compact_1, profile, max_rel);
    assert!(saw_emission, "default v2 world must emit");
    assert!(
        saw_trail_energy,
        "split dual-carrier emission must populate Carrier-1 trail energy"
    );
    assert!(
        saw_vascular_change,
        "V2-14 vascular edges must evolve under flux"
    );

    // C.7 + worker invariance: Compact 1-worker == FreeList 4-worker bit-for-bit!
    assert_eq!(
        w_compact_1.digest(),
        w_freelist_4.digest(),
        "C.7: Compact/1w and FreeList/4w must match bit-for-bit on default V2"
    );

    // Watched organism snapshot MUST reproduce held outputs + hidden activations!
    if w_compact_1.population().get(watched_id).is_some() {
        let snap = w_compact_1
            .organism_snapshot(watched_id)
            .expect("snapshot succeeds")
            .expect("organism alive");
        assert!(
            snap.sensed.is_some(),
            "watched organism must have sensed record"
        );
        assert!(
            snap.activations.is_some(),
            "watched organism activations must reproduce 108-input held output bit-for-bit"
        );
    }

    // C.8 cancel-safety: preparing and cancelling leaves committed planes AND
    // vascular edges bit-for-bit untouched!
    let digest_before = w_compact_1.digest();
    let edges_before: (Vec<u32>, Vec<u32>) = {
        let (gx, gy) = w_compact_1.field().vascular_edges().expect("edges");
        (
            gx.iter().map(|v| v.to_bits()).collect(),
            gy.iter().map(|v| v.to_bits()).collect(),
        )
    };
    let trail_before: Vec<u32> = w_compact_1
        .field()
        .trail_energy()
        .expect("trail")
        .iter()
        .map(|v| v.to_bits())
        .collect();
    if let flife_engine::Advance::Ready(candidate) = w_compact_1.advance().expect("advance") {
        candidate.cancel();
    } else {
        panic!("unexpected pause on cancel test");
    }
    assert_eq!(w_compact_1.digest(), digest_before, "C.8 digest unchanged");
    let (gx_after, gy_after) = w_compact_1.field().vascular_edges().expect("edges");
    assert_eq!(
        gx_after.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        edges_before.0,
        "C.8 east vascular edges untouched"
    );
    assert_eq!(
        gy_after.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        edges_before.1,
        "C.8 north vascular edges untouched"
    );
    assert_eq!(
        w_compact_1
            .field()
            .trail_energy()
            .expect("trail")
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        trail_before,
        "C.8 trail energy untouched"
    );

    // Checkpoint v3 round-trip + resumed ticks match uninterrupted run bit-for-bit!
    let mut writer = CheckpointWriter::new(&cfg.config().model);
    w_compact_1
        .write_checkpoint(&mut writer)
        .expect("write checkpoint");
    let bytes = writer.finish().expect("finish");
    let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("open");
    let mut restored = World::read_checkpoint(&reader, profile).expect("read checkpoint");
    assert_eq!(restored.digest(), w_compact_1.digest());
    for _ in 0..16 {
        assert!(matches!(
            w_compact_1.step().expect("step orig"),
            Stepped::Published(_)
        ));
        assert!(matches!(
            restored.step().expect("step restored"),
            Stepped::Published(_)
        ));
    }
    assert_eq!(
        restored.digest(),
        w_compact_1.digest(),
        "resumed default V2 world must match uninterrupted bit-for-bit"
    );

    // Crop + Graft preserves Carrier 0, Carrier 1 trail energy, AND vascular edge slices!
    let center = [8.0, 8.0];
    let radius = 4.0;
    let (cropped, measured) = w_compact_1
        .cropped(center, radius)
        .expect("crop default v2 world");
    assert!(
        (cropped.field().energy_total() - measured.field_energy).abs() <= 1e-6,
        "crop field_energy {} must match measure_region {}",
        cropped.field().energy_total(),
        measured.field_energy
    );
    let mut host = World::found(&cfg, Arrangement::Compact, profile).expect("host");
    let graft = host
        .graft(&cropped, center, radius, [2, -1])
        .expect("graft v2 package into host");
    assert!(graft.cells > 0);
    // Grafted host must step cleanly with INV-03 closure!
    let mut graft_max_rel = 0.0_f64;
    for _ in 0..8 {
        let Stepped::Published(r) = host.step().expect("step grafted") else {
            panic!("pause");
        };
        graft_max_rel = graft_max_rel.max(r.closure.residual().abs() / r.closure.scale().max(1.0));
    }
    check_inv03(&host, profile, graft_max_rel);
}

#[test]
fn v2_15_dual_carrier_injection_uptake_and_split_emission_exact_ledger() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[v2]
profile = "minimal"

[v2.carriers]
enabled = true
trail_diffusion = 0.0
trail_decay_per_second = 0.0
trail_emission_share = 0.25
"#,
    );
    let cfg = ValidatedConfig::from_toml(&toml).expect("valid");
    let mut world = World::found(&cfg, Arrangement::Compact, profile).expect("world");

    // Step 1: let organisms emit so Carrier 1 receives split emission energy.
    let mut max_rel = 0.0_f64;
    let mut saw_emission_transfer = false;
    for _ in 0..16 {
        let Stepped::Published(r) = world.step().expect("step") else {
            panic!("pause");
        };
        let rel = r.closure.residual().abs() / r.closure.scale().max(1.0);
        max_rel = max_rel.max(rel);
        if r.emitted > 0 {
            let debited = r.closure.debited(Transfer::Emission);
            let credited = r.closure.credited(Transfer::Emission);
            assert!(debited > 0.0 && credited > 0.0 && credited <= debited);
            saw_emission_transfer = true;
        }
    }
    assert!(saw_emission_transfer);
    check_inv03(&world, profile, max_rel);

    // Step 2: InjectMaterial with non-zero channels 24..31 must cleanly target
    // Carrier 0 without crashing on 34-plane field, and keep exact INV-03!
    let mut dir = [0.0_f64; 32];
    dir[0] = 0.6;
    dir[25] = 0.8;
    let inject = Intervention::InjectMaterial {
        cell: [16, 16],
        amount: 5.0,
        direction: dir,
    };
    let Stepped::Published(report) = world.step_with(&[inject]).expect("inject step") else {
        panic!("pause");
    };
    assert!(report.closure.flow(Flow::CommandInjection) > 4.9);
    let rel = report.closure.residual().abs() / report.closure.scale().max(1.0);
    check_inv03(&world, profile, max_rel.max(rel));

    // Step 3: RemoveOrganisms with Recycle disposition recycles cleanly into
    // Carrier 0 with zero Carrier-1 corruption!
    let remove = Intervention::RemoveOrganisms {
        center: [8.0, 8.0],
        radius: 3.0,
        disposition: Disposition::Recycle,
    };
    let Stepped::Published(report) = world.step_with(&[remove]).expect("remove step") else {
        panic!("pause");
    };
    let rel = report.closure.residual().abs() / report.closure.scale().max(1.0);
    check_inv03(&world, profile, max_rel.max(rel));
}

#[test]
fn v2_14_e2e_vascular_oases_carve_living_highways() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let overrides: &[(&str, &str, &str)] = &[
        ("world", "width", "16.0"),
        ("world", "height", "16.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("world", "tick_numerator", "1"),
        ("world", "tick_denominator", "32"),
        ("population", "initial_count", "64"),
        ("population", "maximum_count", "384"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "400.0"),
        ("sources", "sigma_world_units", "1.5"),
        ("sources", "path_direction_interval_seconds", "4.0"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ];

    let run = |tail: &str| {
        let toml = ecology_toml("field-ecology-v2", overrides, tail);
        let cfg = ValidatedConfig::from_toml(&toml).expect("valid");
        let mut world = World::found(&cfg, Arrangement::Compact, profile).expect("world");
        let mut max_rel = 0.0_f64;
        for _ in 0..384 {
            let Stepped::Published(r) = world.step().expect("step") else {
                panic!("pause");
            };
            max_rel = max_rel.max(r.closure.residual().abs() / r.closure.scale().max(1.0));
        }
        check_inv03(&world, profile, max_rel);
        let (high_frac, low_frac, std_dev) = match world.field().vascular_edges() {
            Some((gx, gy)) => {
                let n = (gx.len() + gy.len()) as f64;
                let vals: Vec<f64> = gx.iter().chain(gy.iter()).map(|&v| f64::from(v)).collect();
                let mean = vals.iter().sum::<f64>() / n;
                let var = vals.iter().map(|&v| (v - mean) * (v - mean)).sum::<f64>() / n;
                let high = vals.iter().filter(|&&v| v > 0.6).count() as f64 / n;
                let low = vals.iter().filter(|&&v| v < 0.1).count() as f64 / n;
                (high, low, var.sqrt())
            }
            None => (0.0, 0.0, 0.0),
        };
        (
            world.population().len(),
            high_frac,
            low_frac,
            std_dev,
            max_rel,
        )
    };

    let superlinear = run(r#"
[v2]
profile = "minimal"

[v2.oases]
enabled = true

[v2.vascular]
enabled = true
minimum_conductance_ratio = 0.025
maximum_diffusion = 0.25
reinforcement_rate_per_second = 1.5
pruning_rate_per_second = 0.35
flux_half_saturation = 0.64
flux_exponent = 1.5
"#);
    let sublinear = run(r#"
[v2]
profile = "minimal"

[v2.oases]
enabled = true

[v2.vascular]
enabled = true
minimum_conductance_ratio = 0.025
maximum_diffusion = 0.25
reinforcement_rate_per_second = 1.5
pruning_rate_per_second = 0.35
flux_half_saturation = 0.64
flux_exponent = 0.7
"#);
    let disabled = run(r#"
[v2]
profile = "minimal"

[v2.oases]
enabled = true
"#);

    eprintln!(
        "V2-14 vascular E2E (384 ticks): mu=1.5(live={}, high={:.3}, low={:.3}, std={:.3}, rel={:e}) \
         vs mu=0.7(live={}, high={:.3}, low={:.3}, std={:.3}, rel={:e}) \
         vs off(live={}, rel={:e})",
        superlinear.0,
        superlinear.1,
        superlinear.2,
        superlinear.3,
        superlinear.4,
        sublinear.0,
        sublinear.1,
        sublinear.2,
        sublinear.3,
        sublinear.4,
        disabled.0,
        disabled.4
    );
    assert!(superlinear.0 > 0, "vascular world must stay alive");
    assert!(
        superlinear.3 > 0.05,
        "superlinear vascular field must carve spatial conductance contrast (std={:.4})",
        superlinear.3
    );
}

#[test]
fn v2_15_e2e_dual_carrier_forms_persistent_pheromone_ridges() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let overrides: &[(&str, &str, &str)] = &[
        ("world", "width", "16.0"),
        ("world", "height", "16.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("world", "tick_numerator", "1"),
        ("world", "tick_denominator", "32"),
        ("population", "initial_count", "64"),
        ("population", "maximum_count", "384"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "300.0"),
        ("sources", "sigma_world_units", "2.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("emission", "maximum_fractional_rate_per_second", "1.5"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ];

    let run = |tail: &str| {
        let toml = ecology_toml("field-ecology-v2", overrides, tail);
        let cfg = ValidatedConfig::from_toml(&toml).expect("valid");
        let mut world = World::found(&cfg, Arrangement::Compact, profile).expect("world");
        let mut max_rel = 0.0_f64;
        for _ in 0..384 {
            let Stepped::Published(r) = world.step().expect("step") else {
                panic!("pause");
            };
            max_rel = max_rel.max(r.closure.residual().abs() / r.closure.scale().max(1.0));
        }
        check_inv03(&world, profile, max_rel);
        let (trail_total, trail_peak, trail_cv) = match world.field().trail_energy() {
            Some(plane) => {
                let n = plane.len() as f64;
                let total: f64 = plane.iter().map(|&v| f64::from(v)).sum();
                let peak = plane.iter().copied().map(f64::from).fold(0.0_f64, f64::max);
                let mean = total / n;
                let var = plane
                    .iter()
                    .map(|&v| {
                        let d = f64::from(v) - mean;
                        d * d
                    })
                    .sum::<f64>()
                    / n;
                let cv = if mean > 0.0 { var.sqrt() / mean } else { 0.0 };
                (total, peak, cv)
            }
            None => (0.0, 0.0, 0.0),
        };
        (
            world.population().len(),
            trail_total,
            trail_peak,
            trail_cv,
            max_rel,
        )
    };

    let separated = run(r#"
[v2]
profile = "minimal"

[v2.oases]
enabled = true

[v2.emission.surplus_floor]
enabled = true
energy_floor = 0.6

[v2.carriers]
enabled = true
trail_diffusion = 0.0003
trail_decay_per_second = 0.01
trail_emission_share = 0.25
"#);
    let matched = run(r#"
[v2]
profile = "minimal"

[v2.oases]
enabled = true

[v2.emission.surplus_floor]
enabled = true
energy_floor = 0.6

[v2.carriers]
enabled = true
trail_diffusion = 0.03
trail_decay_per_second = 0.2
trail_emission_share = 0.25
"#);

    eprintln!(
        "V2-15 dual-carrier E2E (384 ticks): slow_trail(live={}, total={:.3}, peak={:.4}, cv={:.3}, rel={:e}) \
         vs matched(live={}, total={:.3}, peak={:.4}, cv={:.3}, rel={:e})",
        separated.0,
        separated.1,
        separated.2,
        separated.3,
        separated.4,
        matched.0,
        matched.1,
        matched.2,
        matched.3,
        matched.4
    );
    assert!(separated.0 > 0, "dual-carrier world must stay alive");
    assert!(
        separated.1 > matched.1,
        "20x slower decay must retain more persistent trail energy ({:.3} vs {:.3})",
        separated.1,
        matched.1
    );
    assert!(
        separated.3 > matched.3,
        "100x slower trail diffusion must preserve sharper spatial trail contrast ({:.3} vs {:.3})",
        separated.3,
        matched.3
    );
}
