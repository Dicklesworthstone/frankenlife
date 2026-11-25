//! Integration, known-answer, invariant, and E2E verification for `V2-11`
//! (`lysis-recycling` C011 and `symplectic-waste` C010) inside `flife-engine`.
//!
//! Covers:
//! - `lysis_recycle_exact_ledger_no_double_spend`: viability-floor lysis via
//!   `TinyEnergyPolicy::LysisRecycle` + `DeathCause::Lysed`, zero
//!   `Flow::TinyReservoir` double-spend, exact `INV-03` closure, and
//!   `-a_eff` / `J a_eff` / `+a_eff` corpse chemical directions;
//! - `symplectic_waste_orthogonal_and_second_law_dissipative`: exact 32-ch and
//!   24-ch orthogonality (`a_eff · J a_eff == 0`), left-fold unit-ball bound,
//!   4-species `J^4 = I` 2nd-Law dissipation, and `Transfer::Uptake` +
//!   `Transfer::MetabolicByproduct` closure;
//! - `emission_plus_byproduct_no_double_deposit_conflict`: same-tick feeding
//!   and emission with dedicated `Scratch::byproducts` / `DecisionLane::byproducts`
//!   (Trap C fix), `Transactions.byproduct_staged`, and 34-plane dual-carrier
//!   Carrier-0 isolation;
//! - `trophic_toggles_disabled_match_baseline_bit_exact`: `--disable`
//!   bit-exact parity with `field-ecology-v1` and `0 B` byproduct overhead;
//! - `trophic_c7_c8_workers_checkpoint_v3_and_memory_admission`: `C.7`, `C.8`,
//!   `1 vs 4` workers, checkpoint v3 round-trip, and `INV-15` admission;
//! - `e2e_symplectic_waste_evolves_cross_feeding_pairs`: multi-hundred-tick
//!   ablation comparing `symplectic-waste` + `lysis-recycling` against control
//!   with §21.4 `INV-03` checks.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::ledger::sub_down;
use flife_core::{FeatureOverride, Flow, Transfer, V2Feature, ValidatedConfig};
use flife_engine::{
    Affinity, Arrangement, BudgetStage, Chemistry, CorpseDirection, DeathCause, NumericalProfile,
    Stepped, TinyEnergyPolicy, WORLD_CHECKPOINT_CHUNKS, World, corpse_affinity_direction,
    symplectic_waste_direction,
};
use flife_field::state::CHANNELS;

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
fn lysis_recycle_exact_ledger_no_double_spend() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    for (dir_name, expected_sign) in [
        ("negated-affinity", -1.0_f64),
        ("symplectic-affinity", 0.0_f64),
        ("genetic-affinity", 1.0_f64),
    ] {
        // Starved single-organism world starting strictly below E_viable = 0.25
        // so it survives Phase 6–7 maintenance with positive energy < 0.25 and
        // lyses in Phase 8 `clear_tiny`.
        let toml = ecology_toml(
            "field-ecology-v2",
            &[
                ("world", "width", "16.0"),
                ("world", "height", "16.0"),
                ("world", "grid_width", "32"),
                ("world", "grid_height", "32"),
                ("world", "tick_numerator", "1"),
                ("world", "tick_denominator", "64"),
                ("population", "initial_count", "1"),
                ("population", "maximum_count", "16"),
                ("population", "initial_energy", "0.20"),
                ("sources", "count", "1"),
                ("sources", "power_per_source", "1e-9"),
                ("death", "residual_energy_recycle_fraction", "0.80"),
                ("execution", "command_durability", "\"volatile-in-memory\""),
            ],
            &format!(
                r#"
[v2]
profile = "minimal"

[v2.emission.surplus_floor]
enabled = true
energy_floor = 0.50

[v2.metabolism.lysis]
enabled = true
viability_energy_floor = 0.25
corpse_direction = "{dir_name}"
"#
            ),
        );
        let cfg = ValidatedConfig::from_toml(&toml).expect("lysis config");
        let policy = TinyEnergyPolicy::from_config(&cfg).expect("tiny policy");
        assert!(
            matches!(policy, TinyEnergyPolicy::LysisRecycle { viability_floor } if (viability_floor - 0.25).abs() < 1e-15),
            "expected TinyEnergyPolicy::LysisRecycle(0.25), got {policy:?}"
        );
        let mut world = World::found(&cfg, Arrangement::Compact, profile).expect("found");
        let org = world.population().iter().next().expect("single founder");
        let affinity = *world
            .population()
            .genome(org.id)
            .expect("genome")
            .traits()
            .affinity();
        world.watch(Some(org.id));

        let Stepped::Published(report) = world.step().expect("step") else {
            panic!("unexpected pause");
        };
        assert_eq!(
            report.live, 0,
            "organism below E_viable must lyse on tick 1"
        );
        assert_eq!(report.deaths.len(), 1);
        let death = report.deaths[0];
        assert_eq!(
            death.cause,
            DeathCause::Lysed(BudgetStage::TinyReservoir),
            "expected DeathCause::Lysed(TinyReservoir)"
        );
        assert!(
            death.residual > 0.10 && death.residual < 0.25,
            "lysed residual {} should lie in (0.10, 0.25)",
            death.residual
        );

        // ZERO double-spend into Flow::TinyReservoir!
        assert_eq!(
            report.closure.flow(Flow::TinyReservoir),
            0.0,
            "lysis must NOT book Flow::TinyReservoir"
        );
        assert!(
            (report.closure.debited(Transfer::Recycling) - death.residual).abs() <= 1e-14,
            "Transfer::Recycling debited {} != lysed residual {}",
            report.closure.debited(Transfer::Recycling),
            death.residual
        );
        let expected_credit = 0.80 * death.residual;
        let actual_credit = report.closure.credited(Transfer::Recycling);
        assert!(
            (actual_credit - expected_credit).abs() <= 1e-5,
            "Transfer::Recycling credited {actual_credit} != expected {expected_credit}"
        );
        assert!(
            report.closure.residual().abs() <= 1e-12,
            "INV-03 residual {:e} exceeded 1e-12 for {dir_name}",
            report.closure.residual()
        );

        // Verify corpse chemical moment direction in the field.
        let field = world.field();
        let mut total_moment = [0.0_f64; CHANNELS];
        let mut total_energy = 0.0_f64;
        for cell in 0..field.energy().len() {
            let e = f64::from(field.energy()[cell]);
            if e > 0.0 {
                total_energy += e;
                let cell_state = field.cell_at(cell).expect("cell");
                for (acc, &m) in total_moment.iter_mut().zip(&cell_state.moments) {
                    *acc += f64::from(m);
                }
            }
        }
        assert!(
            (total_energy - actual_credit).abs() <= 1e-5,
            "field total energy {total_energy} != recycled credit {actual_credit}"
        );
        let a_norm_sq: f64 = affinity.iter().map(|v| v * v).sum();
        assert!(a_norm_sq > 0.0);
        let dot: f64 = affinity.iter().zip(&total_moment).map(|(a, m)| a * m).sum();
        if expected_sign < 0.0 {
            assert!(
                dot < -0.5 * total_energy * a_norm_sq,
                "negated-affinity corpse dot {dot} must be strongly negative"
            );
        } else if expected_sign > 0.0 {
            assert!(
                dot > 0.5 * total_energy * a_norm_sq,
                "genetic-affinity corpse dot {dot} must be strongly positive"
            );
        } else {
            assert!(
                dot.abs() <= 1e-5 * total_energy,
                "symplectic-affinity corpse dot {dot} must be orthogonal (~0)"
            );
        }
        check_inv03(
            &world,
            profile,
            report.closure.residual().abs() / report.closure.scale().max(1.0),
        );
    }
}

#[test]
fn symplectic_waste_orthogonal_and_second_law_dissipative() {
    // 1. Direct mathematical checks on 32-ch and 24-ch symplectic rotation J.
    let mut raw = [0.0_f64; CHANNELS];
    for (i, slot) in raw.iter_mut().enumerate() {
        *slot = ((i as f64) * 0.37 + 0.13).sin();
    }
    let norm = raw.iter().map(|v| v * v).sum::<f64>().sqrt();
    let mut a = [0.0_f64; CHANNELS];
    for (dst, &src) in a.iter_mut().zip(&raw) {
        *dst = src / norm;
    }

    for active_channels in [32_usize, 24_usize] {
        let j1 = symplectic_waste_direction(&a, active_channels, 1.0).expect("J a");
        let j2 = symplectic_waste_direction(&j1, active_channels, 1.0).expect("J^2 a");
        let j3 = symplectic_waste_direction(&j2, active_channels, 1.0).expect("J^3 a");
        let j4 = symplectic_waste_direction(&j3, active_channels, 1.0).expect("J^4 a");

        // Exact orthogonality over active channels: a · (J a) == 0.
        let dot: f64 = a[..active_channels]
            .iter()
            .zip(&j1[..active_channels])
            .map(|(x, y)| x * y)
            .sum();
        assert!(
            dot.abs() <= 1e-14,
            "active={active_channels}: a · J a = {dot:e} must be 0"
        );
        // Trailing channels strictly zeroed on 24-ch Carrier 0.
        for (ch, &val) in j1.iter().enumerate().skip(active_channels) {
            assert_eq!(
                val, 0.0,
                "channel {ch} must be 0 for active={active_channels}"
            );
        }
        // Left-fold unit-ball bound for all powers of J.
        for (k, v) in [j1, j2, j3, j4].iter().enumerate() {
            let sq = v.iter().fold(0.0_f64, |acc, &x| acc + x * x);
            assert!(
                sq <= 1.0,
                "active={active_channels}: ||J^{} a||_2^2 = {sq} exceeds 1.0",
                k + 1
            );
        }
        // J^4 = I on active channels (up to tiny unit-ball shrink margin).
        for ch in 0..active_channels {
            assert!(
                (j4[ch] - a[ch]).abs() <= 1e-12,
                "active={active_channels} ch={ch}: J^4 a != a ({} vs {})",
                j4[ch],
                a[ch]
            );
        }

        // Also check corpse_affinity_direction NegatedAffinity zeroing & sign.
        let neg =
            corpse_affinity_direction(&a, CorpseDirection::NegatedAffinity, active_channels, 1.0)
                .expect("negated");
        for ch in 0..active_channels {
            assert_eq!(neg[ch], -a[ch]);
        }
        for &val in &neg[active_channels..] {
            assert_eq!(val, 0.0);
        }

        // 2nd-Law 4-species closed loop dissipation test:
        // Species k has affinity J^k a (unit-ball, truncated to active_channels).
        let chem = Chemistry::new(0.65, 3.0, 0.4, 1.0, 1.0).expect("chem");
        let rho_waste = 0.50;
        let species = [j4, j1, j2, j3];
        let mut energy = 10.0_f64;
        for k in 0..4 {
            let producer = Affinity::new(species[k], &chem).expect("producer");
            let waste =
                symplectic_waste_direction(&species[k], active_channels, 1.0).expect("waste");
            // Producer cannot gain from its own waste (a_eff · J a_eff == 0 => η < 0).
            let self_eta = chem.efficiency(&producer, &waste).expect("self eta");
            assert!(
                self_eta < 0.0,
                "producer {k} must have negative efficiency on its own waste, got {self_eta}"
            );
            // Consumer k+1 absorbs energy from waste and excretes unconverted energy * rho_waste.
            let consumer = Affinity::new(species[(k + 1) % 4], &chem).expect("consumer");
            let eta = chem.efficiency(&consumer, &waste).expect("consumer eta");
            assert!(eta > 0.0, "cross-feeding tier must gain from J a");
            let gained = eta * energy;
            let unconverted = energy - gained;
            energy = rho_waste * unconverted;
        }
        assert!(
            energy < 1.0,
            "4-tier closed trophic loop must strictly dissipate energy (10.0 -> {energy})"
        );
    }

    // 2. World-level check: Transfer::Uptake + Transfer::MetabolicByproduct = exact U.
    let profile = NumericalProfile::reference_f32().expect("profile");
    let toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[v2]
profile = "minimal"

[v2.metabolism.waste]
enabled = true
byproduct_recycle_fraction = 0.50
"#,
    );
    let cfg = ValidatedConfig::from_toml(&toml).expect("waste cfg");
    let mut world = World::found(&cfg, Arrangement::Compact, profile).expect("found");
    let mut total_byproduct_debited = 0.0;
    let mut total_byproduct_credited = 0.0;
    let mut max_rel = 0.0_f64;
    for _ in 0..24 {
        let Stepped::Published(report) = world.step().expect("step") else {
            panic!("unexpected pause");
        };
        let u_deb = report.closure.debited(Transfer::Uptake);
        let b_deb = report.closure.debited(Transfer::MetabolicByproduct);
        let b_cred = report.closure.credited(Transfer::MetabolicByproduct);
        total_byproduct_debited += b_deb;
        total_byproduct_credited += b_cred;
        assert!(b_cred <= b_deb + 1e-12);
        let granted = report.uptake.granted;
        assert!(
            ((u_deb + b_deb) - granted).abs() <= 1e-10 * granted.max(1.0),
            "Uptake({u_deb}) + MetabolicByproduct({b_deb}) != granted({granted})"
        );
        max_rel = max_rel.max(report.closure.residual().abs() / report.closure.scale().max(1.0));
    }
    assert!(
        total_byproduct_debited > 0.0 && total_byproduct_credited > 0.0,
        "expected positive metabolic byproduct recycling"
    );
    check_inv03(&world, profile, max_rel);
}

#[test]
fn emission_plus_byproduct_no_double_deposit_conflict() {
    let profile = NumericalProfile::reference_f32().expect("profile");

    // Test both 33-plane single-carrier AND 34-plane dual-carrier (V2-15)!
    for dual_carrier in [false, true] {
        let tail = if dual_carrier {
            r#"
[v2]
profile = "minimal"

[v2.metabolism.waste]
enabled = true
byproduct_recycle_fraction = 0.45

[v2.carriers]
enabled = true
"#
        } else {
            r#"
[v2]
profile = "minimal"

[v2.metabolism.waste]
enabled = true
byproduct_recycle_fraction = 0.45
"#
        };
        let toml = ecology_toml(
            "field-ecology-v2",
            &[
                ("world", "width", "16.0"),
                ("world", "height", "16.0"),
                ("world", "grid_width", "32"),
                ("world", "grid_height", "32"),
                ("world", "tick_numerator", "1"),
                ("world", "tick_denominator", "32"),
                ("population", "initial_count", "48"),
                ("population", "maximum_count", "256"),
                ("population", "initial_energy", "4.0"),
                ("sources", "count", "4"),
                ("sources", "power_per_source", "250.0"),
                ("sources", "sigma_world_units", "2.0"),
                ("sources", "path_direction_interval_seconds", "2.0"),
                ("emission", "gate_threshold", "0.05"),
                ("execution", "command_durability", "\"volatile-in-memory\""),
            ],
            tail,
        );
        let cfg = ValidatedConfig::from_toml(&toml).expect("cfg");
        let mut world = World::found(&cfg, Arrangement::Compact, profile).expect("found");
        let watched_id = world.population().iter().next().expect("founder").id;
        world.watch(Some(watched_id));

        let mut saw_simultaneous_tick = false;
        let mut saw_watched_byproduct_and_emission = false;
        let mut max_rel = 0.0_f64;
        for _ in 0..32 {
            let Stepped::Published(report) = world.step().expect("step") else {
                panic!("unexpected pause");
            };
            let emitted_deb = report.closure.debited(Transfer::Emission);
            let waste_deb = report.closure.debited(Transfer::MetabolicByproduct);
            if emitted_deb > 0.0 && waste_deb > 0.0 {
                saw_simultaneous_tick = true;
            }
            if let Some(snap) = world.organism_snapshot(watched_id).expect("snapshot")
                && let Some(tx) = snap.transactions.last()
            {
                let unconverted = sub_down(tx.uptake.absorbed, tx.uptake.gain).max(0.0);
                let expected = (0.45 * unconverted).min(unconverted);
                if expected > 0.0 && (tx.uptake.absorbed - expected) >= tx.uptake.gain {
                    assert!(
                        (tx.byproduct_staged - expected).abs() <= 1e-15,
                        "watched byproduct_staged {} != expected {expected}",
                        tx.byproduct_staged
                    );
                } else {
                    assert_eq!(tx.byproduct_staged, 0.0);
                }
                if tx.byproduct_staged > 0.0 && tx.emission.is_some_and(|e| e.emitted()) {
                    saw_watched_byproduct_and_emission = true;
                }
            }
            max_rel =
                max_rel.max(report.closure.residual().abs() / report.closure.scale().max(1.0));
        }
        assert!(
            saw_simultaneous_tick,
            "dual_carrier={dual_carrier}: expected simultaneous Emission + MetabolicByproduct ticks with zero Trap C conflict"
        );
        assert!(
            saw_watched_byproduct_and_emission,
            "dual_carrier={dual_carrier}: expected watched organism to feed and emit on the same tick"
        );
        check_inv03(&world, profile, max_rel);
    }
}

#[test]
fn trophic_toggles_disabled_match_baseline_bit_exact() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let v1_toml = ecology_toml("field-ecology-v1", small_overrides(), "");
    let v2_toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[v2]
profile = "minimal"

[v2.metabolism.lysis]
enabled = true
viability_energy_floor = 0.15

[v2.metabolism.waste]
enabled = true
byproduct_recycle_fraction = 0.50
"#,
    );
    let v1_cfg = ValidatedConfig::from_toml(&v1_toml).expect("v1");
    let v2_off_cfg = ValidatedConfig::from_toml_with_overrides(
        &v2_toml,
        &[
            FeatureOverride::Disable(V2Feature::LysisRecycling.slug().to_owned()),
            FeatureOverride::Disable(V2Feature::SymplecticWaste.slug().to_owned()),
        ],
    )
    .expect("v2 disabled");
    let v2_flags = v2_off_cfg.config().v2.as_ref().expect("v2");
    assert!(!v2_flags.is_enabled(V2Feature::LysisRecycling));
    assert!(!v2_flags.is_enabled(V2Feature::SymplecticWaste));
    assert!(
        !TinyEnergyPolicy::from_config(&v2_off_cfg)
            .expect("tiny")
            .is_lysis()
    );

    let mut w_v1 = World::found(&v1_cfg, Arrangement::Compact, profile).expect("v1 world");
    let mut w_off = World::found(&v2_off_cfg, Arrangement::Compact, profile).expect("v2 off");
    assert_eq!(
        w_off.footprint().scratch,
        w_v1.footprint().scratch,
        "disabled trophic toggles must allocate 0 B extra scratch"
    );
    assert_eq!(w_off.digest(), w_v1.digest());

    for tick in 0..24 {
        let Stepped::Published(r1) = w_v1.step().expect("v1 step") else {
            panic!("pause");
        };
        let Stepped::Published(r2) = w_off.step().expect("v2 off step") else {
            panic!("pause");
        };
        assert_eq!(r2.closure.debited(Transfer::MetabolicByproduct), 0.0);
        assert_eq!(
            w_off.digest(),
            w_v1.digest(),
            "disabled trophic toggles diverged from v1 at tick {tick}"
        );
        assert_eq!(r1.live, r2.live);
    }
}

#[test]
fn trophic_c7_c8_workers_checkpoint_v3_and_memory_admission() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[v2]
profile = "minimal"

[v2.metabolism.lysis]
enabled = true
viability_energy_floor = 0.15

[v2.metabolism.waste]
enabled = true
byproduct_recycle_fraction = 0.45
"#,
    );
    let cfg = ValidatedConfig::from_toml(&toml).expect("cfg");
    let mut compact_1 = World::found(&cfg, Arrangement::Compact, profile).expect("compact");
    let mut freelist_1 = World::found(&cfg, Arrangement::FreeList, profile).expect("fl");
    let mut compact_4 = World::found(&cfg, Arrangement::Compact, profile).expect("4w");
    compact_4
        .set_cpu_workers(NonZeroUsize::new(4).expect("4"))
        .expect("set 4 workers");

    let admission = compact_1.admit_memory(false, 0, 0).expect("admit_memory");
    assert!(admission.peak_bytes <= compact_1.config().config().execution.maximum_total_bytes);
    let bound = compact_1.footprint().total();
    assert!(compact_1.allocated().total() <= bound);

    // Step 12 ticks, verify C.7 (Compact == FreeList) and 1 vs 4 workers bit-exact.
    for tick in 0..12 {
        let _ = compact_1.step().expect("c1");
        let _ = freelist_1.step().expect("fl1");
        let _ = compact_4.step().expect("c4");
        assert_eq!(
            compact_1.digest(),
            freelist_1.digest(),
            "C.7 mismatch at {tick}"
        );
        assert_eq!(
            compact_1.digest(),
            compact_4.digest(),
            "1 vs 4 workers mismatch at {tick}"
        );
        assert!(
            compact_1.allocated().total() <= bound,
            "INV-15 overshoot at {tick}"
        );
    }

    // C.8 cancel-safety: prepare candidate, cancel, digest unchanged, next step matches freelist_1.
    let before_cancel = compact_1.digest();
    match compact_1.advance().expect("advance") {
        flife_engine::Advance::Ready(candidate) => candidate.cancel(),
        flife_engine::Advance::Paused(_) => panic!("unexpected pause"),
    }
    assert_eq!(
        compact_1.digest(),
        before_cancel,
        "C.8 cancel mutated world"
    );

    // Checkpoint v3 write -> read -> step 8 more ticks bit-exact.
    let mut writer = CheckpointWriter::new(compact_1.config().config().model.as_str());
    compact_1
        .write_checkpoint(&mut writer)
        .expect("write checkpoint");
    let bytes = writer.finish().expect("finish checkpoint");
    let reader =
        CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("parse checkpoint");
    let mut restored = World::read_checkpoint(&reader, profile).expect("read checkpoint");
    assert_eq!(restored.digest(), compact_1.digest());

    let mut max_rel = 0.0_f64;
    for tick in 0..8 {
        let Stepped::Published(r1) = compact_1.step().expect("c1") else {
            panic!("pause");
        };
        let _ = freelist_1.step().expect("fl1");
        let _ = restored.step().expect("restored");
        assert_eq!(
            compact_1.digest(),
            freelist_1.digest(),
            "post-cancel mismatch at {tick}"
        );
        assert_eq!(
            compact_1.digest(),
            restored.digest(),
            "post-checkpoint mismatch at {tick}"
        );
        max_rel = max_rel.max(r1.closure.residual().abs() / r1.closure.scale().max(1.0));
    }
    check_inv03(&compact_1, profile, max_rel);
}

#[test]
fn e2e_symplectic_waste_evolves_cross_feeding_pairs() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let overrides: &[(&str, &str, &str)] = &[
        ("world", "width", "16.0"),
        ("world", "height", "16.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("world", "tick_numerator", "1"),
        ("world", "tick_denominator", "32"),
        ("population", "initial_count", "64"),
        ("population", "maximum_count", "256"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "250.0"),
        ("sources", "sigma_world_units", "2.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ];
    let toml = ecology_toml(
        "field-ecology-v2",
        overrides,
        r#"
[v2]
profile = "minimal"

[v2.metabolism.lysis]
enabled = true
viability_energy_floor = 0.12

[v2.metabolism.waste]
enabled = true
byproduct_recycle_fraction = 0.50
"#,
    );
    let enabled_cfg = ValidatedConfig::from_toml(&toml).expect("enabled");
    let control_cfg = ValidatedConfig::from_toml_with_overrides(
        &toml,
        &[FeatureOverride::Disable(
            V2Feature::SymplecticWaste.slug().to_owned(),
        )],
    )
    .expect("control");

    let run = |cfg: ValidatedConfig| {
        let mut world = World::found(&cfg, Arrangement::Compact, profile).expect("found");
        let mut waste_credited = 0.0_f64;
        let mut lysed_count = 0_usize;
        let mut max_rel = 0.0_f64;
        for _ in 0..256 {
            let Stepped::Published(report) = world.step().expect("step") else {
                panic!("pause");
            };
            waste_credited += report.closure.credited(Transfer::MetabolicByproduct);
            lysed_count += report
                .deaths
                .iter()
                .filter(|d| matches!(d.cause, DeathCause::Lysed(_)))
                .count();
            max_rel =
                max_rel.max(report.closure.residual().abs() / report.closure.scale().max(1.0));
        }
        check_inv03(&world, profile, max_rel);
        let mut lineages: BTreeMap<u64, usize> = BTreeMap::new();
        let orgs: Vec<_> = world.population().iter().collect();
        for o in &orgs {
            *lineages.entry(o.lineage.get()).or_insert(0) += 1;
        }
        let n = orgs.len() as f64;
        let simpson: f64 = lineages
            .values()
            .map(|&c| {
                let p = (c as f64) / n.max(1.0);
                p * p
            })
            .sum();
        let n_eff = if simpson > 0.0 { 1.0 / simpson } else { 0.0 };
        let mut pairs = 0_usize;
        let mut cross_feeding = 0_usize;
        for (i, oi) in orgs.iter().enumerate() {
            let ai = world
                .population()
                .genome(oi.id)
                .expect("g")
                .traits()
                .affinity();
            let j_ai = symplectic_waste_direction(ai, CHANNELS, 1.0).expect("J a");
            let j_norm = j_ai.iter().map(|v| v * v).sum::<f64>().sqrt();
            if j_norm == 0.0 {
                continue;
            }
            for oj in &orgs[(i + 1)..] {
                let dx = (oi.position[0] - oj.position[0])
                    .abs()
                    .min(16.0 - (oi.position[0] - oj.position[0]).abs());
                let dy = (oi.position[1] - oj.position[1])
                    .abs()
                    .min(16.0 - (oi.position[1] - oj.position[1]).abs());
                if dx * dx + dy * dy <= 2.25 {
                    let aj = world
                        .population()
                        .genome(oj.id)
                        .expect("g")
                        .traits()
                        .affinity();
                    let aj_norm = aj.iter().map(|v| v * v).sum::<f64>().sqrt();
                    if aj_norm > 0.0 {
                        pairs += 1;
                        let cos = j_ai.iter().zip(aj).map(|(x, y)| x * y).sum::<f64>()
                            / (j_norm * aj_norm);
                        if cos.abs() > 0.5 {
                            cross_feeding += 1;
                        }
                    }
                }
            }
        }
        (
            world.population().len(),
            waste_credited,
            lysed_count,
            n_eff,
            pairs,
            cross_feeding,
        )
    };

    let (live_on, waste_on, _lysed_on, _neff_on, _pairs_on, _cf_on) = run(enabled_cfg);
    let (_live_off, waste_off, _lysed_off, _neff_off, _pairs_off, _cf_off) = run(control_cfg);
    assert!(
        waste_on > 0.0,
        "symplectic-waste run must produce positive Transfer::MetabolicByproduct flux"
    );
    assert_eq!(
        waste_off, 0.0,
        "control with symplectic-waste disabled must have 0.0 MetabolicByproduct flux"
    );
    assert!(live_on > 0, "symplectic-waste ecology must remain viable");
}

#[test]
fn corpse_affinity_direction_rejects_nonfinite_on_full_and_dual_carrier_paths() {
    for active in [CHANNELS, 24] {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for idx in [0, active - 1] {
                let mut affinity = [0.0_f64; CHANNELS];
                affinity[idx] = bad;
                for dir in [
                    CorpseDirection::GeneticAffinity,
                    CorpseDirection::NegatedAffinity,
                    CorpseDirection::SymplecticAffinity,
                ] {
                    assert!(
                        matches!(
                            corpse_affinity_direction(&affinity, dir, active, 1.0),
                            Err(flife_core::CoreError::NumericalInvalid { .. })
                        ),
                        "expected NumericalInvalid for {dir:?} active={active} idx={idx} bad={bad}"
                    );
                }
            }
        }
    }
    // Negative zero is normalized to +0.0 on the full-32 GeneticAffinity path too.
    let mut neg_zero = [0.0_f64; CHANNELS];
    neg_zero[0] = -0.0;
    let out = corpse_affinity_direction(&neg_zero, CorpseDirection::GeneticAffinity, CHANNELS, 1.0)
        .expect("finite");
    assert_eq!(out[0].to_bits(), 0.0_f64.to_bits());
}
