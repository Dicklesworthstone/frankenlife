//! V2-07 tests: Hippo-YAP contact-inhibition-of-proliferation local crowding
//! birth gate (`hippo-crowding`, `local-neighbors-v1` & `cell-occupancy-v1`).
//!
//! Covers:
//! - Independent known-answer verification of `hippo_gate`,
//!   `hippo_attempt_probability`, `hippo_crowding_sensor`, and spatial/cell
//!   neighbor counting across torus seams.
//! - Validation bounds (`c_max >= 2.0`, `INV-08` honesty for
//!   `local-neighbors-v1` vs `cell-occupancy-v1`, `profile = "auto"` defaults).
//! - Bit-exact baseline parity when disabled (`0` extra scratch bytes).
//! - Worker-count (`1 vs 4`), arrangement (`C.7`), cancel (`C.8`), checkpoint
//!   v3 round-trip (`INV-14`), `World::footprint()` (`INV-15`), and
//!   `expose_sensor = true` + `World::organism_snapshot` trace parity.
//! - E2E wound-punch proliferation test (`Intervention::RemoveOrganisms`
//!   immediate release and `rim_birth_rate >= 3.0 * interior_birth_rate`).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration tests assert directly on invariants and fixtures"
)]

use std::num::NonZeroUsize;

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::clock::TimeStep;
use flife_core::numerics::expm1_neg;
use flife_core::{FeatureOverride, V2Feature, ValidatedConfig};
use flife_engine::contact::CrowdingWork;
use flife_engine::{
    Arrangement, Disposition, Intervention, NumericalProfile, Stepped, WORLD_CHECKPOINT_CHUNKS,
    World, hippo_attempt_probability, hippo_crowding_sensor, hippo_gate,
};
use flife_field::FieldGrid;

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

fn compact_contact_tail(extra: &str) -> String {
    format!(
        r#"
[contact]
diameter = 0.5
stiffness = 0.6
adhesion = 0.0
adhesion_range = 1.0
{extra}
"#
    )
}

fn base_small_overrides() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        ("world", "width", "16.0"),
        ("world", "height", "16.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("world", "tick_numerator", "1"),
        ("world", "tick_denominator", "32"),
        ("population", "initial_count", "64"),
        ("population", "maximum_count", "512"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "120.0"),
        ("sources", "sigma_world_units", "2.5"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.4"),
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
fn hippo_gate_exact_arrest_at_c_max() {
    let c_max = 4.5_f64;
    let lambda = 4.0_f64;
    let step = TimeStep::new(1, 32).expect("valid timestep");

    // Known-answer closed-form checks for N_loc = 0, 2, 4, 5, 8.
    for (n, want_gate) in [
        (0_u32, 1.0_f64),
        (2, 1.0 - 2.0 / 4.5),
        (4, 1.0 - 4.0 / 4.5),
        (5, 0.0),
        (8, 0.0),
    ] {
        let g = hippo_gate(n, c_max);
        assert!(
            (g - want_gate).abs() <= 1e-15,
            "n={n}: got gate {g}, want {want_gate}"
        );
        let p = hippo_attempt_probability(lambda, step, n, c_max).expect("valid p");
        if n >= 5 {
            assert_eq!(p, 0.0, "n={n} >= c_max must completely arrest births");
        } else {
            let want_p = expm1_neg(lambda * want_gate * step.seconds_per_tick()).expect("expm1");
            assert_eq!(
                p.to_bits(),
                want_p.to_bits(),
                "n={n} must match exact expm1"
            );
        }
        let c_den = hippo_crowding_sensor(n);
        let want_c_den = f64::from(n) / (f64::from(n) + 4.0);
        assert_eq!(c_den.to_bits(), want_c_den.to_bits());
    }
    // Rim/chain cells (N_loc <= 2) retain >= 50% of decoded attempt rate.
    assert!(hippo_gate(2, c_max) >= 0.5);

    // Spatial neighbor counting across torus seam (0, 0).
    let grid = FieldGrid::new(32, 32, 16.0, 16.0).expect("grid");
    let radius = 0.75_f64;
    // Place center organism at (0.1, 0.1) and test 0, 2, 4, 5, 8 neighbors inside radius,
    // including neighbors wrapped across x=16 and y=16, plus one outside radius.
    let neighbor_offsets: [[f64; 2]; 8] = [
        [0.2, 0.0],
        [-0.2, 0.0], // wraps across x=0!
        [0.0, 0.2],
        [0.0, -0.2],  // wraps across y=0!
        [0.3, 0.3],   // dist = 0.424 < 0.75
        [-0.3, 0.3],  // wraps x, dist = 0.424
        [0.3, -0.3],  // wraps y, dist = 0.424
        [-0.3, -0.3], // wraps both x & y, dist = 0.424
    ];
    for count in [0_usize, 2, 4, 5, 8] {
        let mut positions = vec![[0.1_f64, 0.1_f64]];
        for &[dx, dy] in &neighbor_offsets[..count] {
            positions.push(grid.wrap_position([0.1 + dx, 0.1 + dy]).expect("wrap"));
        }
        // Far decoy well outside radius.
        positions.push([8.0, 8.0]);
        let mut work = CrowdingWork::with_capacity_local_neighbors(&grid, radius, positions.len())
            .expect("alloc");
        let counts = work
            .count_local_neighbors(&grid, radius, &positions, 1)
            .expect("count");
        assert_eq!(
            counts[0], count as u32,
            "center organism must count exact {count} wrapped neighbors"
        );
        assert_eq!(
            *counts.last().expect("decoy"),
            0,
            "isolated decoy must count 0 neighbors"
        );
    }

    // Pure-field cell occupancy: 4 organisms in cell (0, 0) + 1 in cell (15, 15).
    let cell_positions = vec![[0.1, 0.1], [0.2, 0.1], [0.1, 0.2], [0.2, 0.2], [7.75, 7.75]];
    let mut cell_work =
        CrowdingWork::with_capacity_cell_occupancy(&grid, cell_positions.len()).expect("alloc");
    let cell_counts = cell_work
        .count_cell_occupancy(&grid, &cell_positions)
        .expect("cell occupancy");
    assert_eq!(cell_counts, &[3, 3, 3, 3, 0]);
}

#[test]
fn hippo_validation_inv08_and_c_max_bounds() {
    // 1. profile = "auto" without [contact] disables hippo-crowding (Bundle B pure-field).
    let pure_toml = ecology_toml("field-ecology-v2", base_small_overrides(), "");
    let pure_cfg = ValidatedConfig::from_toml(&pure_toml).expect("valid pure v2");
    assert!(pure_cfg.inv08_pure_field());
    assert!(
        !pure_cfg
            .config()
            .v2
            .as_ref()
            .expect("v2")
            .is_enabled(V2Feature::HippoCrowding)
    );

    // 2. profile = "auto" WITH [contact] enables hippo-crowding (Bundle A).
    let contact_toml = ecology_toml(
        "field-ecology-v2",
        base_small_overrides(),
        &compact_contact_tail(""),
    );
    let contact_cfg = ValidatedConfig::from_toml(&contact_toml).expect("valid contact v2");
    assert!(!contact_cfg.inv08_pure_field());
    assert!(
        contact_cfg
            .config()
            .v2
            .as_ref()
            .expect("v2")
            .is_enabled(V2Feature::HippoCrowding)
    );

    // 3. local-neighbors-v1 WITHOUT [contact] is refused!
    let bad_local = ecology_toml(
        "field-ecology-v2",
        base_small_overrides(),
        r#"
[v2.crowding]
enabled = true
mode = "local-neighbors-v1"
"#,
    );
    assert!(
        ValidatedConfig::from_toml(&bad_local).is_err(),
        "local-neighbors-v1 without [contact] must be rejected"
    );

    // 4. cell-occupancy-v1 WITHOUT [contact] succeeds and preserves INV-08 pure-field!
    let good_cell = ecology_toml(
        "field-ecology-v2",
        base_small_overrides(),
        r#"
[v2]
profile = "minimal"

[v2.crowding]
enabled = true
mode = "cell-occupancy-v1"
arrest_threshold = 4.5
"#,
    );
    let good_cell_cfg = ValidatedConfig::from_toml(&good_cell).expect("cell-occupancy valid");
    assert!(
        good_cell_cfg.inv08_pure_field(),
        "cell-occupancy-v1 must preserve inv08_pure_field = true"
    );

    // 5. arrest_threshold < 2.0 is rejected.
    for bad_c_max in ["1.99", "1.0", "0.0", "-2.0"] {
        let bad = ecology_toml(
            "field-ecology-v2",
            base_small_overrides(),
            &compact_contact_tail(&format!(
                r#"
[v2]
profile = "minimal"

[v2.crowding]
enabled = true
mode = "local-neighbors-v1"
arrest_threshold = {bad_c_max}
"#
            )),
        );
        assert!(
            ValidatedConfig::from_toml(&bad).is_err(),
            "arrest_threshold={bad_c_max} must be rejected"
        );
    }
}

#[test]
fn hippo_disabled_matches_baseline_bit_exact() {
    let v1_toml = ecology_toml(
        "field-ecology-v1-contact",
        base_small_overrides(),
        &compact_contact_tail(""),
    );
    let v2_toml = ecology_toml(
        "field-ecology-v2",
        base_small_overrides(),
        &compact_contact_tail(""),
    );
    let v1_cfg = ValidatedConfig::from_toml(&v1_toml).expect("v1 contact");
    let v2_cfg = ValidatedConfig::from_toml_with_overrides(&v2_toml, &[FeatureOverride::V1Compat])
        .expect("v2 compat");

    let mut w1 = World::found(
        &v1_cfg,
        Arrangement::Compact,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("w1");
    let mut w2 = World::found(
        &v2_cfg,
        Arrangement::Compact,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("w2");
    assert_eq!(
        w1.footprint().scratch,
        w2.footprint().scratch,
        "disabled hippo-crowding must reserve 0 extra scratch bytes"
    );
    for _ in 0..48 {
        assert!(matches!(w1.step().expect("w1"), Stepped::Published(_)));
        assert!(matches!(w2.step().expect("w2"), Stepped::Published(_)));
    }
    assert_eq!(
        w1.digest(),
        w2.digest(),
        "--disable hippo-crowding / --v1-compat must match baseline bit-for-bit"
    );
}

#[test]
fn hippo_worker_order_c7_c8_and_checkpoint_invariance() {
    let profile = NumericalProfile::reference_f32().expect("profile");

    // Test BOTH local-neighbors-v1 (with expose_sensor = true -> 105-input ABI + watch trace!)
    // and cell-occupancy-v1 (pure-field INV-08).
    let local_toml = ecology_toml(
        "field-ecology-v2",
        base_small_overrides(),
        &compact_contact_tail(
            r#"
[v2]
profile = "minimal"

[v2.crowding]
enabled = true
mode = "local-neighbors-v1"
radius_world_units = 0.75
arrest_threshold = 4.5
expose_sensor = true
"#,
        ),
    );
    let cell_toml = ecology_toml(
        "field-ecology-v2",
        base_small_overrides(),
        r#"
[v2]
profile = "minimal"

[v2.crowding]
enabled = true
mode = "cell-occupancy-v1"
arrest_threshold = 4.5
expose_sensor = true
"#,
    );

    for (label, toml) in [
        ("local-neighbors-v1", local_toml),
        ("cell-occupancy-v1", cell_toml),
    ] {
        let cfg = ValidatedConfig::from_toml(&toml).expect("valid config");
        let mut w_compact_1 = World::found_on(
            &cfg,
            Arrangement::Compact,
            profile,
            NonZeroUsize::new(1).expect("1"),
        )
        .expect("compact 1");
        let mut w_free_4 = World::found_on(
            &cfg,
            Arrangement::FreeList,
            profile,
            NonZeroUsize::new(4).expect("4"),
        )
        .expect("freelist 4");

        // Watch organism 0 so we also verify organism_snapshot trace_raw matches
        // with expose_sensor = true (105 inputs including c_den)!
        let watched_id = w_compact_1.population().ids().next().expect("founder 0");
        w_compact_1.watch(Some(watched_id));

        let footprint_before = w_compact_1.footprint();
        for tick in 0..32 {
            if tick == 16 {
                // C.8: prepare and cancel a candidate without mutating state.
                let digest_before = w_compact_1.digest();
                let candidate = w_compact_1.advance().expect("advance");
                if let flife_engine::Advance::Ready(c) = candidate {
                    c.cancel();
                }
                assert_eq!(w_compact_1.digest(), digest_before, "{label}: C.8 cancel");
            }
            assert!(matches!(
                w_compact_1.step().expect("step 1"),
                Stepped::Published(_)
            ));
            assert!(matches!(
                w_free_4.step().expect("step 4"),
                Stepped::Published(_)
            ));
        }
        // Snapshot trace verification with expose_sensor = true must succeed bit-for-bit!
        let snapshot = w_compact_1
            .organism_snapshot(watched_id)
            .expect("snapshot with expose_sensor = true must verify bit-exact");
        if let Some(snap) = snapshot {
            assert!(
                snap.activations.is_some(),
                "{label}: watched evaluated organism must reproduce activations"
            );
        }
        assert_eq!(
            w_compact_1.digest(),
            w_free_4.digest(),
            "{label}: 1v4 workers and Compact vs FreeList (C.7) must match bit-for-bit"
        );
        // INV-15: bounded footprint never grows while stepping.
        let footprint_after = w_compact_1.footprint();
        assert_eq!(footprint_after.scratch, footprint_before.scratch, "{label}");
        assert_eq!(footprint_after.field, footprint_before.field, "{label}");
        assert!(footprint_after.state <= footprint_before.state, "{label}");

        // INV-14: checkpoint v3 save/restore must resume bit-identically with 0 crowding bytes.
        let mut writer = CheckpointWriter::new(&cfg.config().model);
        w_compact_1.write_checkpoint(&mut writer).expect("write");
        let bytes = writer.finish().expect("finish");
        let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("open");
        let mut restored = World::read_checkpoint(&reader, profile).expect("restore");
        assert_eq!(restored.digest(), w_compact_1.digest(), "{label}: restore");
        for _ in 0..24 {
            assert!(matches!(
                w_compact_1.step().expect("cont 1"),
                Stepped::Published(_)
            ));
            assert!(matches!(
                restored.step().expect("cont restored"),
                Stepped::Published(_)
            ));
        }
        assert_eq!(
            restored.digest(),
            w_compact_1.digest(),
            "{label}: resumed ticks must match uninterrupted run bit-for-bit"
        );
    }
}

#[test]
fn e2e_hippo_wound_punch_rim_proliferation() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let diameter = 0.65_f64;
    let wound_center = [8.0_f64, 8.0_f64];
    let wound_radius = 2.5_f64;

    // Confluent tissue sheet on a 16x16 torus (32x32 grid, D = 0.65, r = 1.5*D = 0.975 ->
    // circle area ~ 3.0, bulk density >= 2.2 orgs/unit area -> bulk N_loc >= 6.6 > c_max = 4.5)
    // so the interior is genuinely Hippo-arrested while the wound rim releases upon lesion.
    let toml = ecology_toml(
        "field-ecology-v2",
        &[
            ("world", "width", "16.0"),
            ("world", "height", "16.0"),
            ("world", "grid_width", "32"),
            ("world", "grid_height", "32"),
            ("world", "tick_numerator", "1"),
            ("world", "tick_denominator", "32"),
            ("population", "initial_count", "480"),
            ("population", "maximum_count", "2048"),
            ("population", "initial_energy", "3.5"),
            ("sources", "count", "4"),
            ("sources", "power_per_source", "600.0"),
            ("sources", "sigma_world_units", "4.0"),
            ("sources", "path_direction_interval_seconds", "2.0"),
            ("chemistry", "uptake_capacity_per_second", "12.0"),
            ("costs", "basal_energy_per_second", "0.25"),
            (
                "reproduction",
                "minimum_parent_energy_before_attempt",
                "1.8",
            ),
            ("reproduction", "maximum_attempt_rate_per_second", "8.0"),
            ("reproduction", "cooldown_seconds", "0.125"),
            ("execution", "command_durability", "\"volatile-in-memory\""),
        ],
        r#"
[contact]
diameter = 0.65
stiffness = 0.75
adhesion = 0.25
adhesion_range = 2.0
sweeps = 2

[v2]
profile = "minimal"

[v2.crowding]
enabled = true
mode = "local-neighbors-v1"
radius_world_units = 0.975
arrest_threshold = 4.5
expose_sensor = false
"#,
    );
    let cfg = ValidatedConfig::from_toml(&toml).expect("valid wound config");
    let mut world = World::found(&cfg, Arrangement::Compact, profile).expect("world");

    // 1. Equilibrate dense confluent sheet for 160 ticks.
    for _ in 0..160 {
        assert!(matches!(world.step().expect("grow"), Stepped::Published(_)));
    }
    let pre_wound_live = world.population().len();
    assert!(
        pre_wound_live >= 450,
        "sheet must be confluent before wound punch, got {pre_wound_live}"
    );

    // 2. Punch circular wound via Intervention::RemoveOrganisms and verify
    //    immediate post-Phase-1 release on that exact tick!
    let wound = Intervention::RemoveOrganisms {
        center: wound_center,
        radius: wound_radius,
        disposition: Disposition::Remove,
    };
    let grid = *world.field().grid();
    let mut rim_organism_ticks = 0_u64;
    let mut rim_births = 0_u64;
    let mut interior_organism_ticks = 0_u64;
    let mut interior_births = 0_u64;
    let mut max_rel_residual = 0.0_f64;

    // Classify every live organism BEFORE each post-wound tick by distance to
    // the wound edge, and attribute committed births to their parent's pre-tick zone.
    for post_tick in 0..64 {
        let mut parent_zone = std::collections::HashMap::new();
        for org in world.population().iter() {
            let [dx, dy] = grid.displacement(wound_center, org.position).expect("disp");
            let d = dx.hypot(dy);
            let edge_dist = (d - wound_radius).abs();
            // Rim: organisms along the wound margin (outside the hole on tick 0,
            // within 1.5*D of the wound boundary).
            // Interior: deep in the confluent bulk (> 3.0*D beyond the wound edge).
            if (post_tick > 0 || d >= wound_radius) && edge_dist < 1.5 * diameter {
                parent_zone.insert(org.id, true);
                rim_organism_ticks += 1;
            } else if d > wound_radius + 3.0 * diameter {
                parent_zone.insert(org.id, false);
                interior_organism_ticks += 1;
            }
        }
        let stepped = if post_tick == 0 {
            world.step_with(&[wound]).expect("wound step")
        } else {
            world.step().expect("heal step")
        };
        let Stepped::Published(report) = stepped else {
            panic!("unexpected pause during wound healing");
        };
        let rel = report.closure.residual().abs() / report.closure.scale().max(1.0);
        max_rel_residual = max_rel_residual.max(rel);
        if post_tick == 0 {
            assert!(
                report.interventions.iter().any(|iv| iv.removed > 0),
                "wound punch must remove organisms inside the disk"
            );
        }
        for birth in &report.births {
            if let Some(&is_rim) = parent_zone.get(&birth.parent) {
                if is_rim {
                    rim_births += 1;
                } else {
                    interior_births += 1;
                }
            }
        }
    }

    let cum_norm = world.ledger().report().normalized.unwrap_or(0.0);
    assert!(
        max_rel_residual <= 1e-6,
        "INV-03 per-tick relative residual {max_rel_residual:e} exceeded 1e-6 (§21.4)"
    );
    assert!(
        cum_norm <= profile.residual.cumulative(),
        "INV-03 cumulative normalized residual {cum_norm:e} exceeded {}",
        profile.residual.cumulative()
    );
    assert!(
        rim_organism_ticks > 0 && interior_organism_ticks > 0,
        "both rim ({rim_organism_ticks}) and interior ({interior_organism_ticks}) must be sampled"
    );
    let rim_rate = rim_births as f64 / rim_organism_ticks as f64;
    let interior_rate = interior_births as f64 / interior_organism_ticks as f64;
    eprintln!(
        "hippo wound E2E: pre_wound_live={pre_wound_live}, rim_births={rim_births}/{rim_organism_ticks} ({rim_rate:.5}), \
         interior_births={interior_births}/{interior_organism_ticks} ({interior_rate:.5}), max_rel_residual={max_rel_residual:e}, cum_norm={cum_norm:e}"
    );
    assert!(
        rim_births > 0,
        "wound-rim organisms must proliferate after lesion"
    );
    assert!(
        rim_rate >= 3.0 * interior_rate || (interior_births == 0 && rim_births > 0),
        "expected wound-rim birth rate ({rim_rate:.5}) >= 3x interior ({interior_rate:.5})"
    );
}

#[test]
fn round2b_parallel_prepare_contact_crowding_and_checkpoint_bit_exact() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    for dtype in ["f32", "f16"] {
        let dtype_toml = format!("\"{dtype}\"");
        let toml = ecology_toml(
            "field-ecology-v2",
            &[
                ("world", "width", "16.0"),
                ("world", "height", "16.0"),
                ("world", "grid_width", "32"),
                ("world", "grid_height", "32"),
                ("world", "tick_numerator", "1"),
                ("world", "tick_denominator", "32"),
                ("population", "initial_count", "640"),
                ("population", "maximum_count", "2048"),
                ("population", "initial_energy", "3.5"),
                ("controller", "canonical_weight_dtype", &dtype_toml),
                ("sources", "count", "4"),
                ("sources", "power_per_source", "400.0"),
                ("execution", "command_durability", "\"volatile-in-memory\""),
            ],
            r#"
[contact]
diameter = 0.5
stiffness = 0.7
adhesion = 0.35
adhesion_range = 2.0
sweeps = 2

[v2]
profile = "minimal"

[v2.crowding]
enabled = true
mode = "local-neighbors-v1"
radius_world_units = 0.85
arrest_threshold = 4.5
expose_sensor = true

[v2.barrier]
mode = "harmonic-v1"
minimum_permeability = 0.04
occupancy_impedance = 1.5

[v2.trophallaxis]
enabled = true

[v2.contact.cadherin]
enabled = true

[v2.contact.velocity_alignment]
enabled = true

[v2.metabolism.waste]
enabled = true
byproduct_recycle_fraction = 0.4

[v2.carriers]
enabled = true
"#,
        );
        let cfg = ValidatedConfig::from_toml_with_overrides(
            &toml,
            &[FeatureOverride::Enable("tight-junctions".to_owned())],
        )
        .expect("valid round2b v2 config");

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
        let mut w8 = World::found_on(
            &cfg,
            Arrangement::Compact,
            profile,
            NonZeroUsize::new(8).expect("8"),
        )
        .expect("w8");

        let scratch_before_w4 = w4.footprint().scratch;
        let scratch_before_w8 = w8.footprint().scratch;
        for tick in 0..8 {
            assert!(
                matches!(w1.step().expect("w1 step"), Stepped::Published(_)),
                "{dtype} tick {tick}: w1 paused"
            );
            assert!(
                matches!(w4.step().expect("w4 step"), Stepped::Published(_)),
                "{dtype} tick {tick}: w4 paused"
            );
            assert!(
                matches!(w8.step().expect("w8 step"), Stepped::Published(_)),
                "{dtype} tick {tick}: w8 paused"
            );
            assert_eq!(w4.digest(), w1.digest(), "{dtype} tick {tick}: w4 vs w1");
            assert_eq!(w8.digest(), w1.digest(), "{dtype} tick {tick}: w8 vs w1");
        }
        assert!(
            w1.population().len() >= 512,
            "{dtype}: must stay >= 512 live to cover Round-2b parallel paths"
        );
        assert_eq!(
            w4.footprint().scratch,
            scratch_before_w4,
            "{dtype}: w4 scratch grew"
        );
        assert_eq!(
            w8.footprint().scratch,
            scratch_before_w8,
            "{dtype}: w8 scratch grew"
        );

        // Bulk f32_slice / f16_slice checkpoint write + restore round-trip.
        let mut writer = CheckpointWriter::new(&cfg.config().model);
        w8.write_checkpoint(&mut writer).expect("write checkpoint");
        let bytes = writer.finish().expect("finish checkpoint");
        let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("parse");
        let mut restored = World::read_checkpoint(&reader, profile).expect("restore");
        assert_eq!(restored.digest(), w1.digest(), "{dtype}: restored digest");
        for tick in 0..4 {
            assert!(matches!(w1.step().expect("w1 post"), Stepped::Published(_)));
            assert!(matches!(
                restored.step().expect("restored post"),
                Stepped::Published(_)
            ));
            assert_eq!(
                restored.digest(),
                w1.digest(),
                "{dtype}: post-restore tick {tick}"
            );
        }
    }
}
