//! Known-answer, invariant, modularity, and E2E verification for `V2-13`:
//! - Passive exact-quantum (`2^-30`) kin trophallaxis (`trophallaxis`, `C013`);
//! - Decoupled 4-D cadherin adhesion tag & cohesiveness (`cadherin-4d`, `C023`);
//! - Dissipative speed-non-increasing kin velocity alignment (`velocity-alignment`, `C019`).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::num::NonZeroUsize;

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::ledger::{Flow, Transfer};
use flife_core::{CoreError, FeatureOverride, V2Feature, ValidatedConfig};
use flife_engine::contact::Bodies;
use flife_engine::{
    Advance, Arrangement, ContactRules, ContactScratch, Intervention, NumericalProfile, Stepped,
    Sweep1Context, Sweep1Output, TROPHALLAXIS_QUANTUM, TrophallaxisSummary,
    TrophallaxisSweepParams, VelocityAlignmentSweepParams, WORLD_CHECKPOINT_CHUNKS, World,
    align_velocity_dissipative, cadherin_pair_weight, pair_trophallaxis_quanta,
    relax_sweep1_with_context, unit_affinity,
};
use flife_field::FieldGrid;
use flife_genome::AFFINITY_TERMS;

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
        ("controller", "hidden_widths", "[8]"),
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

fn unit_aff_channel(channel: usize) -> [f32; AFFINITY_TERMS] {
    let mut a = [0.0_f64; AFFINITY_TERMS];
    a[channel] = 1.0;
    unit_affinity(&a)
}

fn unit_aff_with_cosine(cos_val: f64) -> [f32; AFFINITY_TERMS] {
    let mut a = [0.0_f64; AFFINITY_TERMS];
    a[0] = cos_val;
    a[1] = (1.0 - cos_val * cos_val).max(0.0).sqrt();
    unit_affinity(&a)
}

fn test_contact_rules() -> ContactRules {
    ContactRules {
        diameter: 0.5,
        stiffness: 0.75,
        adhesion: 0.35,
        range: 2.0,
        sweeps: 3,
        inhibits: false,
        inhibition: 0.0,
        repulsion: 0.0,
    }
}

fn local_polar_order(world: &World, radius: f64) -> (f64, usize) {
    let grid = *world.field().grid();
    let width = grid.world_width();
    let height = grid.world_height();
    let half_w = 0.5 * width;
    let half_h = 0.5 * height;
    let mut moving: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for org in world.population().iter() {
        let speed = org.velocity[0].hypot(org.velocity[1]);
        if speed > 1.0e-6 {
            moving.push((
                org.position,
                [org.velocity[0] / speed, org.velocity[1] / speed],
            ));
        }
    }
    let r2 = radius * radius;
    let mut sum_phi = 0.0_f64;
    let mut pairs = 0_usize;
    for (i, &(pos_i, hat_i)) in moving.iter().enumerate() {
        let mut sx = hat_i[0];
        let mut sy = hat_i[1];
        let mut count = 1_usize;
        for (j, &(pos_j, hat_j)) in moving.iter().enumerate() {
            if i == j {
                continue;
            }
            let mut dx = pos_j[0] - pos_i[0];
            if dx >= half_w {
                dx -= width;
            } else if dx < -half_w {
                dx += width;
            }
            let mut dy = pos_j[1] - pos_i[1];
            if dy >= half_h {
                dy -= height;
            } else if dy < -half_h {
                dy += height;
            }
            if dx * dx + dy * dy <= r2 {
                sx += hat_j[0];
                sy += hat_j[1];
                count += 1;
            }
        }
        if count >= 2 {
            sum_phi += sx.hypot(sy) / (count as f64);
            pairs += 1;
        }
    }
    if pairs == 0 {
        (0.0, 0)
    } else {
        (sum_phi / (pairs as f64), pairs)
    }
}

#[test]
fn trophallaxis_exact_quantum_conservation_and_donor_cap() {
    let grid = FieldGrid::new(64, 64, 32.0, 32.0).expect("grid");
    let rules = test_contact_rules();
    let aff = unit_aff_channel(0);

    // 1. 1-donor / 10-starving-recipient star graph: high conductance forces
    //    Pass-1 export cap `cap = f_max * (E_donor - E_reserve)`.
    let mut positions = vec![[16.0, 16.0]];
    let mut energies = vec![2.0_f64];
    for k in 0..10 {
        let theta = (k as f64) * core::f64::consts::TAU / 10.0;
        let dist = 0.6 * rules.diameter;
        positions.push([16.0 + dist * theta.cos(), 16.0 + dist * theta.sin()]);
        energies.push(0.10_f64);
    }
    let n = positions.len();
    let ids: Vec<u64> = (1..=(n as u64)).collect();
    let alive = vec![true; n];
    let affinities = vec![aff; n];
    let tr_params = TrophallaxisSweepParams {
        dt: 1.0 / 32.0,
        conductance_per_second: 64.0,
        transfer_efficiency: 0.95,
        reserve_energy: 0.25,
        maximum_export_fraction: 0.25,
        kin_cosine_exponent: 4,
        kin_threshold: 0.5,
    };
    let mut scratch = ContactScratch::default();
    let mut disp = Vec::new();
    let mut deltas = Vec::new();
    let mut summary = TrophallaxisSummary::default();

    relax_sweep1_with_context(
        &grid,
        &rules,
        Bodies {
            positions: &positions,
            ids: &ids,
            alive: &alive,
            affinities: Some(&affinities),
        },
        &Sweep1Context {
            trophallaxis: Some(tr_params),
            energies: Some(&energies),
            ..Sweep1Context::default()
        },
        Sweep1Output {
            displacements: &mut disp,
            next_polarities: None,
            neighbor_h0: None,
            trophallaxis_delta_quanta: Some(&mut deltas),
            trophallaxis_summary: Some(&mut summary),
            next_velocities: None,
        },
        &mut scratch,
        1,
    )
    .expect("star trophallaxis");

    assert!(summary.total_out_quanta > 0, "expected nonzero export");
    assert!(summary.total_in_quanta > 0, "expected nonzero import");
    assert!(summary.total_tax_quanta > 0, "expected nonzero 5% tax");
    assert_eq!(
        summary.total_out_quanta,
        summary.total_in_quanta + summary.total_tax_quanta,
        "exact 0-bit integer quantum conservation violated"
    );
    let net_quanta: i128 = deltas.iter().map(|&k| i128::from(k)).sum();
    assert_eq!(
        net_quanta,
        -i128::from(summary.total_tax_quanta),
        "sum of per-organism deltas must equal -total_tax_quanta to 0 bits"
    );
    assert_eq!(
        (summary.debited() - summary.credited()).to_bits(),
        summary.tax().to_bits(),
        "exact 2^-30 quantum debited - credited must equal tax to 0 bits"
    );
    let before_sum: f64 = energies.iter().sum();
    let after_sum: f64 = energies
        .iter()
        .zip(&deltas)
        .map(|(&e, &k)| e + (k as f64) * TROPHALLAXIS_QUANTUM)
        .sum();
    let star_residual = ((after_sum - before_sum) + summary.tax()).abs();
    assert!(
        star_residual <= 1e-14 * before_sum.max(1.0),
        "star trophallaxis INV-03 residual {star_residual:e} exceeded 1e-14"
    );
    let expected_cap = tr_params.maximum_export_fraction * (energies[0] - tr_params.reserve_energy);
    let donor_exported = (-deltas[0] as f64) * TROPHALLAXIS_QUANTUM;
    let donor_after = energies[0] + (deltas[0] as f64) * TROPHALLAXIS_QUANTUM;
    assert!(
        donor_exported <= expected_cap + 1e-15,
        "donor exported {donor_exported} > cap {expected_cap}"
    );
    assert!(
        donor_after >= tr_params.reserve_energy,
        "donor energy {donor_after} fell below reserve {}",
        tr_params.reserve_energy
    );
    for (idx, &k) in deltas.iter().enumerate().skip(1) {
        assert!(k > 0, "recipient {idx} must receive positive quanta");
    }

    // 2. Clonal sharing across differentiated V2-12 sisters (`h_0 = 0` vs `h_0 = 1`):
    //    because trophallaxis reads GENETIC `traits.affinity()` (not `a_eff(h_0)`),
    //    differentiated clonal sisters freely pipe energy inward alongside Notch-Delta.
    let mut notch_out = Vec::new();
    let mut sister_deltas = Vec::new();
    let mut sister_summary = TrophallaxisSummary::default();
    relax_sweep1_with_context(
        &grid,
        &rules,
        Bodies {
            positions: &[[10.0, 10.0], [10.35, 10.0]],
            ids: &[1, 2],
            alive: &[true, true],
            affinities: Some(&[aff, aff]),
        },
        &Sweep1Context {
            cell_states_h0: Some(&[0.0_f32, 1.0_f32]),
            trophallaxis: Some(tr_params),
            energies: Some(&[3.0, 0.5]),
            ..Sweep1Context::default()
        },
        Sweep1Output {
            displacements: &mut disp,
            next_polarities: None,
            neighbor_h0: Some(&mut notch_out),
            trophallaxis_delta_quanta: Some(&mut sister_deltas),
            trophallaxis_summary: Some(&mut sister_summary),
            next_velocities: None,
        },
        &mut scratch,
        1,
    )
    .expect("differentiated sisters");
    assert!(
        sister_summary.total_out_quanta > 0 && sister_deltas[0] < 0 && sister_deltas[1] > 0,
        "differentiated clonal sisters (h0=0 vs h0=1) must freely exchange trophallaxis quanta"
    );
    assert_eq!(
        sister_summary.total_out_quanta,
        sister_summary.total_in_quanta + sister_summary.total_tax_quanta
    );

    // 3. 1 vs 4 worker bit-exact determinism over 5,000 bodies (> MINIMUM_BODIES_PER_WORKER).
    let count = 5_000_usize;
    let mut p_pos = Vec::with_capacity(count);
    let mut p_e = Vec::with_capacity(count);
    let mut p_aff = Vec::with_capacity(count);
    for i in 0..count {
        p_pos.push([
            ((i * 73) % 3199) as f64 * 0.01,
            ((i * 197) % 3199) as f64 * 0.01,
        ]);
        p_e.push(0.15 + ((i * 29) % 400) as f64 * 0.01);
        p_aff.push(unit_aff_channel(i % 2));
    }
    let p_ids: Vec<u64> = (1..=(count as u64)).collect();
    let p_alive = vec![true; count];
    let bodies = Bodies {
        positions: &p_pos,
        ids: &p_ids,
        alive: &p_alive,
        affinities: Some(&p_aff),
    };
    let ctx = Sweep1Context {
        trophallaxis: Some(tr_params),
        energies: Some(&p_e),
        ..Sweep1Context::default()
    };
    let (mut d1, mut k1, mut s1) = (Vec::new(), Vec::new(), TrophallaxisSummary::default());
    let (mut d4, mut k4, mut s4) = (Vec::new(), Vec::new(), TrophallaxisSummary::default());
    relax_sweep1_with_context(
        &grid,
        &rules,
        bodies,
        &ctx,
        Sweep1Output {
            displacements: &mut d1,
            next_polarities: None,
            neighbor_h0: None,
            trophallaxis_delta_quanta: Some(&mut k1),
            trophallaxis_summary: Some(&mut s1),
            next_velocities: None,
        },
        &mut scratch,
        1,
    )
    .expect("1 worker");
    relax_sweep1_with_context(
        &grid,
        &rules,
        bodies,
        &ctx,
        Sweep1Output {
            displacements: &mut d4,
            next_polarities: None,
            neighbor_h0: None,
            trophallaxis_delta_quanta: Some(&mut k4),
            trophallaxis_summary: Some(&mut s4),
            next_velocities: None,
        },
        &mut scratch,
        4,
    )
    .expect("4 workers");
    assert!(
        s1.total_out_quanta > 0,
        "expected active transfers in 5k cloud"
    );
    assert_eq!(s1, s4, "1 vs 4 worker trophallaxis summary mismatch");
    assert_eq!(k1, k4, "1 vs 4 worker per-body quanta mismatch");
    assert_eq!(
        (s1.debited() - s1.credited()).to_bits(),
        s1.tax().to_bits(),
        "5k cloud debited - credited must equal tax to 0 bits"
    );
    let cloud_before: f64 = p_e.iter().sum();
    let cloud_after: f64 = p_e
        .iter()
        .zip(&k1)
        .map(|(&e, &k)| e + (k as f64) * TROPHALLAXIS_QUANTUM)
        .sum();
    let cloud_rel = ((cloud_after - cloud_before) + s1.tax()).abs() / cloud_before.max(1.0);
    assert!(
        cloud_rel <= 1e-14,
        "5k cloud pure trophallaxis INV-03 residual {cloud_rel:e} exceeded 1e-14"
    );
    for i in 0..count {
        assert_eq!(d1[i].map(f64::to_bits), d4[i].map(f64::to_bits));
    }

    // 4. Full-world F32 INV-03 (§21.4) + exact `< 1e-15` per-tick trophallaxis
    //    transfer balance + watched `Transactions.trophallaxis_delta` with
    //    `epigenetic-recurrence + trophallaxis` active together.
    let profile = NumericalProfile::reference_f32().expect("profile");
    let world_overrides: &[(&str, &str, &str)] = &[
        ("world", "width", "16.0"),
        ("world", "height", "16.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("world", "tick_numerator", "1"),
        ("world", "tick_denominator", "32"),
        ("population", "initial_count", "32"),
        ("population", "maximum_count", "256"),
        ("population", "initial_energy", "4.5"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "200.0"),
        ("sources", "sigma_world_units", "2.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "4.0"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.0",
        ),
        ("reproduction", "cooldown_seconds", "0.25"),
        ("reproduction", "maximum_attempt_rate_per_second", "8.0"),
        ("controller", "hidden_widths", "[8]"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ];
    let toml = ecology_toml(
        "field-ecology-v2",
        world_overrides,
        r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.55
adhesion_range = 2.0
sweeps = 2

[v2]
profile = "minimal"

[v2.differentiation]
enabled = true
state_width = 2
affinity_rotation_enabled = true

[v2.trophallaxis]
enabled = true
conductance_per_second = 8.0
transfer_efficiency = 0.95
reserve_energy = 0.25
maximum_export_fraction = 0.25
kin_cosine_exponent = 4
kin_threshold = 0.5
"#,
    );
    let cfg = ValidatedConfig::from_toml(&toml).expect("world trophallaxis cfg");
    let mut w1 = World::found(&cfg, Arrangement::Compact, profile).expect("w1");
    let mut w4 = World::found(&cfg, Arrangement::Compact, profile).expect("w4");
    w4.set_cpu_workers(NonZeroUsize::new(4).expect("4"))
        .expect("4 workers");
    let watched_id = w1.population().ids().next().expect("watched");
    let food_dir = w1
        .population()
        .genome(watched_id)
        .expect("genome")
        .traits()
        .affinity()
        .map(|x| x * 0.99);
    let watched_pos = w1.population().get(watched_id).expect("org").position;
    let watched_cell = [
        ((watched_pos[0] / w1.field().grid().cell_width()).floor() as u32)
            .min(w1.field().grid().cells_x() - 1),
        ((watched_pos[1] / w1.field().grid().cell_height()).floor() as u32)
            .min(w1.field().grid().cells_y() - 1),
    ];
    w1.watch(Some(watched_id));

    let mut max_rel = 0.0_f64;
    let mut total_tr_debited = 0.0_f64;
    let mut watched_saw_delta = false;
    for tick in 0..96 {
        let interventions = if tick % 2 == 0 {
            vec![Intervention::InjectMaterial {
                cell: watched_cell,
                amount: 6.0,
                direction: food_dir,
            }]
        } else {
            Vec::new()
        };
        let Stepped::Published(r1) = w1.step_with(&interventions).expect("w1") else {
            panic!("pause");
        };
        let _ = w4.step_with(&interventions).expect("w4");
        assert_eq!(w1.digest(), w4.digest(), "world 1 vs 4 mismatch at {tick}");
        let deb = r1.closure.debited(Transfer::Trophallaxis);
        let cred = r1.closure.credited(Transfer::Trophallaxis);
        let tax = r1.closure.flow(Flow::TrophallaxisLoss);
        assert!(
            ((deb - cred) - tax).abs() <= 1e-15,
            "tick {tick}: trophallaxis ledger mismatch: deb={deb:e}, cred={cred:e}, tax={tax:e}"
        );
        let rel = r1.closure.residual().abs() / r1.closure.scale().max(1.0);
        max_rel = max_rel.max(rel);
        total_tr_debited += deb;
    }
    check_inv03(&w1, profile, max_rel);
    assert!(
        total_tr_debited > 0.0,
        "expected live clonal trophallaxis transfers in differentiated world"
    );
    if let Some(snap) = w1.organism_snapshot(watched_id).expect("snap") {
        if let Some(last_tx) = snap.transactions.last() {
            assert!(
                (last_tx.closing - snap.organism.energy).abs() <= 1e-15,
                "watched tx.closing ({}) must match post-trophallaxis survivor.energy ({})",
                last_tx.closing,
                snap.organism.energy
            );
        }
        if snap
            .transactions
            .iter()
            .any(|tx| tx.trophallaxis_delta != 0.0)
        {
            watched_saw_delta = true;
        }
    }
    eprintln!(
        "[v2-13] trophallaxis_exact_quantum_conservation: cloud_rel={cloud_rel:e}, world_max_rel={max_rel:e}, total_tr_debited={total_tr_debited:.6}, watched_saw_delta={watched_saw_delta}"
    );
}

#[test]
fn trophallaxis_cos4_blocks_non_kin() {
    let grid = FieldGrid::new(32, 32, 16.0, 16.0).expect("grid");
    let rules = test_contact_rules();
    let tr_params = TrophallaxisSweepParams {
        dt: 1.0 / 32.0,
        conductance_per_second: 16.0,
        transfer_efficiency: 0.95,
        reserve_energy: 0.25,
        maximum_export_fraction: 0.25,
        kin_cosine_exponent: 4,
        kin_threshold: 0.5,
    };
    let mut scratch = ContactScratch::default();
    let mut disp = Vec::new();
    let mut deltas = Vec::new();
    let mut summary = TrophallaxisSummary::default();
    let positions = [[8.0, 8.0], [8.3, 8.0]];

    // Unrelated pair (`cos = 0.2 < 0.5`): strictly 0 quanta exchanged.
    let a0 = unit_aff_channel(0);
    let a_nonkin = unit_aff_with_cosine(0.2);
    relax_sweep1_with_context(
        &grid,
        &rules,
        Bodies {
            positions: &positions,
            ids: &[1, 2],
            alive: &[true, true],
            affinities: Some(&[a0, a_nonkin]),
        },
        &Sweep1Context {
            trophallaxis: Some(tr_params),
            energies: Some(&[3.0, 0.2]),
            ..Sweep1Context::default()
        },
        Sweep1Output {
            displacements: &mut disp,
            next_polarities: None,
            neighbor_h0: None,
            trophallaxis_delta_quanta: Some(&mut deltas),
            trophallaxis_summary: Some(&mut summary),
            next_velocities: None,
        },
        &mut scratch,
        1,
    )
    .expect("non-kin");
    assert_eq!(summary, TrophallaxisSummary::default());
    assert_eq!(deltas, vec![0, 0]);

    // Borderline below threshold (`cos = 0.49`): still strictly 0 quanta.
    let a_below = unit_aff_with_cosine(0.49);
    relax_sweep1_with_context(
        &grid,
        &rules,
        Bodies {
            positions: &positions,
            ids: &[1, 2],
            alive: &[true, true],
            affinities: Some(&[a0, a_below]),
        },
        &Sweep1Context {
            trophallaxis: Some(tr_params),
            energies: Some(&[3.0, 0.2]),
            ..Sweep1Context::default()
        },
        Sweep1Output {
            displacements: &mut disp,
            next_polarities: None,
            neighbor_h0: None,
            trophallaxis_delta_quanta: Some(&mut deltas),
            trophallaxis_summary: Some(&mut summary),
            next_velocities: None,
        },
        &mut scratch,
        1,
    )
    .expect("below threshold");
    assert_eq!(summary, TrophallaxisSummary::default());
    assert_eq!(deltas, vec![0, 0]);

    // Kin pair (`cos = 0.8 >= 0.5`): exact match against `pair_trophallaxis_quanta`.
    let a_kin = unit_aff_with_cosine(0.8);
    let exact_cos: f64 = a0
        .iter()
        .zip(&a_kin)
        .map(|(x, y)| f64::from(*x) * f64::from(*y))
        .sum();
    let expected = pair_trophallaxis_quanta(
        3.0,
        1.0,
        0.2,
        0.3_f64.abs().hypot(0.0),
        rules.diameter * rules.range,
        exact_cos,
        &tr_params,
    );
    assert!(expected.0 > 0 && expected.1 > 0 && expected.2 > 0);
    relax_sweep1_with_context(
        &grid,
        &rules,
        Bodies {
            positions: &positions,
            ids: &[1, 2],
            alive: &[true, true],
            affinities: Some(&[a0, a_kin]),
        },
        &Sweep1Context {
            trophallaxis: Some(tr_params),
            energies: Some(&[3.0, 0.2]),
            ..Sweep1Context::default()
        },
        Sweep1Output {
            displacements: &mut disp,
            next_polarities: None,
            neighbor_h0: None,
            trophallaxis_delta_quanta: Some(&mut deltas),
            trophallaxis_summary: Some(&mut summary),
            next_velocities: None,
        },
        &mut scratch,
        1,
    )
    .expect("kin pair");
    assert_eq!(deltas, vec![-expected.0, expected.1]);
    assert_eq!(summary.total_out_quanta, expected.0 as u64);
    assert_eq!(summary.total_in_quanta, expected.1 as u64);
    assert_eq!(summary.total_tax_quanta, expected.2 as u64);

    // Donor at `E_reserve` exports 0 quanta.
    relax_sweep1_with_context(
        &grid,
        &rules,
        Bodies {
            positions: &positions,
            ids: &[1, 2],
            alive: &[true, true],
            affinities: Some(&[a0, a0]),
        },
        &Sweep1Context {
            trophallaxis: Some(tr_params),
            energies: Some(&[tr_params.reserve_energy, 0.05]),
            ..Sweep1Context::default()
        },
        Sweep1Output {
            displacements: &mut disp,
            next_polarities: None,
            neighbor_h0: None,
            trophallaxis_delta_quanta: Some(&mut deltas),
            trophallaxis_summary: Some(&mut summary),
            next_velocities: None,
        },
        &mut scratch,
        1,
    )
    .expect("at reserve");
    assert_eq!(summary, TrophallaxisSummary::default());
    assert_eq!(deltas, vec![0, 0]);
}

#[test]
fn cadherin_4d_orthog_diet_adhesion_and_zero_norm_guard() {
    let grid = FieldGrid::new(32, 32, 16.0, 16.0).expect("grid");
    let rules = test_contact_rules();
    let positions = [[8.0, 8.0], [8.75, 8.0]];
    let a1 = unit_aff_channel(0);
    let a2 = unit_aff_channel(1); // strictly orthogonal: cos(a1, a2) == 0.0
    let mut scratch = ContactScratch::default();
    let mut disp_base = Vec::new();

    // 1. Baseline: orthogonal diets cannot adhere (`disp == 0`).
    relax_sweep1_with_context(
        &grid,
        &rules,
        Bodies {
            positions: &positions,
            ids: &[1, 2],
            alive: &[true, true],
            affinities: Some(&[a1, a2]),
        },
        &Sweep1Context::default(),
        Sweep1Output {
            displacements: &mut disp_base,
            next_polarities: None,
            neighbor_h0: None,
            trophallaxis_delta_quanta: None,
            trophallaxis_summary: None,
            next_velocities: None,
        },
        &mut scratch,
        1,
    )
    .expect("baseline orthog");
    assert_eq!(disp_base[0], [0.0, 0.0]);
    assert_eq!(disp_base[1], [0.0, 0.0]);

    // 2. Cadherin-4d with identical 4-D tags `c1 == c2` pulls symmetrically even
    //    when diets are orthogonal — and works with `affinities: None` (8x speedup).
    let c = [0.5_f64, 0.5, 0.5, 0.5];
    assert!((cadherin_pair_weight(&c, 0.8, &c, 0.8) - 0.8).abs() <= 1e-15);
    let mut disp_cad = Vec::new();
    relax_sweep1_with_context(
        &grid,
        &rules,
        Bodies {
            positions: &positions,
            ids: &[1, 2],
            alive: &[true, true],
            affinities: None,
        },
        &Sweep1Context {
            cadherin_tags: Some(&[c, c]),
            cadherin_cohesiveness: Some(&[0.8, 0.8]),
            ..Sweep1Context::default()
        },
        Sweep1Output {
            displacements: &mut disp_cad,
            next_polarities: None,
            neighbor_h0: None,
            trophallaxis_delta_quanta: None,
            trophallaxis_summary: None,
            next_velocities: None,
        },
        &mut scratch,
        1,
    )
    .expect("cadherin orthog");
    assert!(
        disp_cad[0][0] > 0.0,
        "orthogonal-diet symbionts with matching cadherin must pull together"
    );
    assert!(
        (disp_cad[0][0] + disp_cad[1][0]).abs() <= 1e-15
            && (disp_cad[0][1] + disp_cad[1][1]).abs() <= 1e-15,
        "cadherin adhesion must obey Newton's 3rd law symmetrically"
    );

    // Non-cohesive (`s = 0.0`) produces zero pull and zero sorting repulsion.
    let sorting_rules = ContactRules {
        repulsion: 0.5,
        ..rules
    };
    let mut disp_zero_s = Vec::new();
    relax_sweep1_with_context(
        &grid,
        &sorting_rules,
        Bodies {
            positions: &positions,
            ids: &[1, 2],
            alive: &[true, true],
            affinities: None,
        },
        &Sweep1Context {
            cadherin_tags: Some(&[c, [1.0, 0.0, 0.0, 0.0]]),
            cadherin_cohesiveness: Some(&[1.0, 0.0]),
            ..Sweep1Context::default()
        },
        Sweep1Output {
            displacements: &mut disp_zero_s,
            next_polarities: None,
            neighbor_h0: None,
            trophallaxis_delta_quanta: None,
            trophallaxis_summary: None,
            next_velocities: None,
        },
        &mut scratch,
        1,
    )
    .expect("zero cohesiveness");
    assert_eq!(disp_zero_s[0], [0.0, 0.0]);
    assert_eq!(disp_zero_s[1], [0.0, 0.0]);

    // 3. Zero-norm cadherin tag `||c|| == 0` is strictly refused.
    let err = relax_sweep1_with_context(
        &grid,
        &rules,
        Bodies {
            positions: &positions,
            ids: &[1, 2],
            alive: &[true, true],
            affinities: None,
        },
        &Sweep1Context {
            cadherin_tags: Some(&[[0.0, 0.0, 0.0, 0.0], c]),
            cadherin_cohesiveness: Some(&[1.0, 1.0]),
            ..Sweep1Context::default()
        },
        Sweep1Output {
            displacements: &mut disp_cad,
            next_polarities: None,
            neighbor_h0: None,
            trophallaxis_delta_quanta: None,
            trophallaxis_summary: None,
            next_velocities: None,
        },
        &mut scratch,
        1,
    )
    .expect_err("zero-norm cadherin tag must be refused");
    assert!(
        matches!(err, CoreError::NumericalInvalid { .. }),
        "expected NumericalInvalid for zero-norm cadherin tag, got {err:?}"
    );

    // 4. Full-world cadherin-4d: OrganismSnapshot accessors, IntroduceOrganisms,
    //    crop/graft `Traits::reseal`, checkpoint v3 round-trip, and digest tag byte 7.
    let profile = NumericalProfile::reference_f32().expect("profile");
    let toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 2.0
sweeps = 2

[v2]
profile = "minimal"

[v2.contact.cadherin]
enabled = true
mutation_std = 0.05
"#,
    );
    let cfg = ValidatedConfig::from_toml(&toml).expect("cadherin cfg");
    let mut world = World::found(&cfg, Arrangement::Compact, profile).expect("world");
    assert!(world.tracks_cadherin());
    assert!(world.population().tracks_cadherin());
    for org in world.population().iter() {
        let snap = world
            .organism_snapshot(org.id)
            .expect("snapshot")
            .expect("alive");
        let via_world = world.cadherin_traits(org.id).expect("world cadherin");
        assert_eq!(snap.cadherin_traits(), Some(via_world));
        assert_eq!(snap.cadherin_tag(), Some(*via_world.tag()));
        assert_eq!(snap.cohesiveness(), Some(via_world.cohesiveness()));
    }
    let _ = world
        .step_with(&[Intervention::IntroduceOrganisms {
            cell: [8, 8],
            count: 4,
            energy: 2.0,
        }])
        .expect("introduce");
    for _ in 0..8 {
        let _ = world.step().expect("step");
    }

    // Crop + Graft preserves cadherin tag & cohesiveness via `Traits::reseal`.
    let (cropped, _) = world.cropped([8.0, 8.0], 7.0).expect("crop");
    assert!(cropped.tracks_cadherin());
    for org in cropped.population().iter() {
        assert_eq!(
            cropped.cadherin_traits(org.id),
            world.cadherin_traits(org.id),
            "crop Traits::reseal must preserve exact cadherin tag & cohesiveness"
        );
    }
    let mut host = World::found(&cfg, Arrangement::Compact, profile).expect("host");
    let _ = host
        .graft(&cropped, [8.0, 8.0], 7.0, [0, 0])
        .expect("graft");
    for org in host.population().iter() {
        assert!(
            host.cadherin_traits(org.id).is_some(),
            "grafted organisms must carry valid cadherin traits"
        );
    }

    // Checkpoint v3 round-trip preserves Population::digest (tag byte 7) & cadherin traits.
    let mut writer = CheckpointWriter::new(cfg.config().model.as_str());
    world.write_checkpoint(&mut writer).expect("write");
    let bytes = writer.finish().expect("finish");
    let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("parse");
    let mut restored = World::read_checkpoint(&reader, profile).expect("read");
    assert_eq!(restored.digest(), world.digest());
    for org in world.population().iter() {
        assert_eq!(
            restored.cadherin_traits(org.id),
            world.cadherin_traits(org.id)
        );
    }
    for _ in 0..6 {
        let _ = world.step().expect("world step");
        let _ = restored.step().expect("restored step");
        assert_eq!(restored.digest(), world.digest());
    }
}

#[test]
fn velocity_alignment_strictly_speed_non_increasing() {
    // 1. Direct unit check over 100 deterministic pseudo-random pairs plus
    //    explicit corner cases (`v_i = 0`, exact anti-parallel cancellation,
    //    subnormal speeds, and high-speed neighbor pull).
    let mut state = 0x5EED_C019_1337_u64;
    let mut next_unit = || -> f64 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let bits = (state >> 11) as f64 / ((1_u64 << 53) as f64);
        2.0 * bits - 1.0
    };
    let mut cases: Vec<([f64; 2], [f64; 2], f64, f64)> = vec![
        ([0.0, 0.0], [5.0, -3.0], 1.0, 0.4),
        ([1.0, 0.0], [-4.0, 0.0], 1.0, 0.2), // exact cancellation: 0.8*1 + 0.2*(-4) == 0
        ([1.0e-154, 0.0], [10.0, 10.0], 1.0, 0.4), // tiny near-subnormal speed
        ([0.5, 0.5], [100.0, 100.0], 2.5, 0.4),
        ([3.0, -4.0], [30.0, 40.0], 1.0, 0.35),
    ];
    for _ in 0..100 {
        let vx = next_unit() * 5.0;
        let vy = next_unit() * 5.0;
        let nx = next_unit() * 25.0;
        let ny = next_unit() * 25.0;
        let w = next_unit().abs() * 4.0 + 0.01;
        let mu = next_unit().abs() * 0.4;
        cases.push(([vx, vy], [nx, ny], w, mu));
    }
    for (idx, &(v_i, sum_v, sum_w, mu)) in cases.iter().enumerate() {
        let v_next = align_velocity_dissipative(v_i, sum_v, sum_w, mu);
        assert!(
            v_next[0].is_finite() && v_next[1].is_finite(),
            "case {idx}: nonfinite velocity {v_next:?}"
        );
        let sq_before = v_i[0] * v_i[0] + v_i[1] * v_i[1];
        let sq_after = v_next[0] * v_next[0] + v_next[1] * v_next[1];
        assert!(
            sq_after <= sq_before,
            "case {idx}: TK-04 violated! sq_after={sq_after:e} > sq_before={sq_before:e}"
        );
    }
    assert_eq!(
        align_velocity_dissipative([0.0, 0.0], [5.0, -3.0], 1.0, 0.4),
        [0.0, 0.0]
    );
    assert_eq!(
        align_velocity_dissipative([1.0, 0.0], [-4.0, 0.0], 1.0, 0.2),
        [0.0, 0.0]
    );

    // 2. Sweep-1 integration across 5,000 bodies: strict `v'·v' <= v·v` for
    //    every body AND 1 vs 4 worker bit-exact determinism.
    let grid = FieldGrid::new(64, 64, 32.0, 32.0).expect("grid");
    let rules = test_contact_rules();
    let count = 5_000_usize;
    let mut pos = Vec::with_capacity(count);
    let mut vels = Vec::with_capacity(count);
    let mut affs = Vec::with_capacity(count);
    for i in 0..count {
        pos.push([
            ((i * 73) % 3199) as f64 * 0.01,
            ((i * 197) % 3199) as f64 * 0.01,
        ]);
        if i % 17 == 0 {
            vels.push([0.0, 0.0]);
        } else {
            vels.push([next_unit() * 4.0, next_unit() * 4.0]);
        }
        affs.push(unit_aff_channel(i % 2));
    }
    let ids: Vec<u64> = (1..=(count as u64)).collect();
    let alive = vec![true; count];
    let bodies = Bodies {
        positions: &pos,
        ids: &ids,
        alive: &alive,
        affinities: Some(&affs),
    };
    let ctx = Sweep1Context {
        velocity_alignment: Some(VelocityAlignmentSweepParams { strength: 0.35 }),
        velocities: Some(&vels),
        ..Sweep1Context::default()
    };
    let mut scratch = ContactScratch::default();
    let (mut d1, mut v1) = (Vec::new(), Vec::new());
    let (mut d4, mut v4) = (Vec::new(), Vec::new());
    relax_sweep1_with_context(
        &grid,
        &rules,
        bodies,
        &ctx,
        Sweep1Output {
            displacements: &mut d1,
            next_polarities: None,
            neighbor_h0: None,
            trophallaxis_delta_quanta: None,
            trophallaxis_summary: None,
            next_velocities: Some(&mut v1),
        },
        &mut scratch,
        1,
    )
    .expect("1 worker");
    relax_sweep1_with_context(
        &grid,
        &rules,
        bodies,
        &ctx,
        Sweep1Output {
            displacements: &mut d4,
            next_polarities: None,
            neighbor_h0: None,
            trophallaxis_delta_quanta: None,
            trophallaxis_summary: None,
            next_velocities: Some(&mut v4),
        },
        &mut scratch,
        4,
    )
    .expect("4 workers");
    let mut any_changed = false;
    for i in 0..count {
        let sq_before = vels[i][0] * vels[i][0] + vels[i][1] * vels[i][1];
        let sq_after = v1[i][0] * v1[i][0] + v1[i][1] * v1[i][1];
        assert!(
            sq_after <= sq_before,
            "body {i}: TK-04 violated in Sweep 1 ({sq_after:e} > {sq_before:e})"
        );
        assert_eq!(d1[i].map(f64::to_bits), d4[i].map(f64::to_bits));
        assert_eq!(v1[i].map(f64::to_bits), v4[i].map(f64::to_bits));
        if v1[i] != vels[i] {
            any_changed = true;
        }
    }
    assert!(any_changed, "expected active velocity alignment updates");
}

#[test]
fn contact_v2_toggles_disabled_match_baseline_bit_exact() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let v1_toml = ecology_toml(
        "field-ecology-v1-contact",
        small_overrides(),
        r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 2.0
"#,
    );
    let v2_all3_toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 2.0

[v2]
profile = "minimal"

[v2.trophallaxis]
enabled = true

[v2.contact.cadherin]
enabled = true

[v2.contact.velocity_alignment]
enabled = true
"#,
    );

    // 1. Disabling all 3 toggles matches `field-ecology-v1-contact` bit-for-bit
    //    with exact `0 B` extra state & scratch allocation (`INV-15`).
    let v1_cfg = ValidatedConfig::from_toml(&v1_toml).expect("v1");
    let v2_off_cfg = ValidatedConfig::from_toml_with_overrides(
        &v2_all3_toml,
        &[
            FeatureOverride::Disable(V2Feature::Trophallaxis.slug().to_owned()),
            FeatureOverride::Disable(V2Feature::Cadherin4d.slug().to_owned()),
            FeatureOverride::Disable(V2Feature::VelocityAlignment.slug().to_owned()),
        ],
    )
    .expect("v2 off");
    let mut w_v1 = World::found(&v1_cfg, Arrangement::Compact, profile).expect("w_v1");
    let mut w_off = World::found(&v2_off_cfg, Arrangement::Compact, profile).expect("w_off");
    assert!(!w_off.tracks_cadherin());
    assert_eq!(w_off.footprint().state, w_v1.footprint().state);
    assert_eq!(w_off.footprint().scratch, w_v1.footprint().scratch);
    assert_eq!(w_off.digest(), w_v1.digest());
    for tick in 0..24 {
        let _ = w_v1.step().expect("v1 step");
        let _ = w_off.step().expect("v2 off step");
        assert_eq!(
            w_off.digest(),
            w_v1.digest(),
            "disabled V2-13 toggles diverged from baseline at tick {tick}"
        );
    }

    // 2. Disabling each toggle independently matches a config without that toggle.
    let no_tr_toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 2.0

[v2]
profile = "minimal"

[v2.contact.cadherin]
enabled = true

[v2.contact.velocity_alignment]
enabled = true
"#,
    );
    let cfg_no_tr_direct = ValidatedConfig::from_toml(&no_tr_toml).expect("direct");
    let cfg_no_tr_override = ValidatedConfig::from_toml_with_overrides(
        &v2_all3_toml,
        &[FeatureOverride::Disable(
            V2Feature::Trophallaxis.slug().to_owned(),
        )],
    )
    .expect("override");
    let mut wa = World::found(&cfg_no_tr_direct, Arrangement::Compact, profile).expect("wa");
    let mut wb = World::found(&cfg_no_tr_override, Arrangement::Compact, profile).expect("wb");
    for tick in 0..12 {
        let _ = wa.step().expect("wa");
        let _ = wb.step().expect("wb");
        assert_eq!(
            wa.digest(),
            wb.digest(),
            "--disable trophallaxis mismatch at tick {tick}"
        );
    }

    // 3. All 3 V2-13 toggles enabled together: `memory_admission` (`INV-15`),
    //    `C.7` (Compact vs FreeList + observer purity), `C.8` (cancel rollback),
    //    1 vs 4 workers, and checkpoint v3 round-trip.
    let cfg_all = ValidatedConfig::from_toml(&v2_all3_toml).expect("all3");
    let mut w_c1 = World::found(&cfg_all, Arrangement::Compact, profile).expect("c1");
    let mut w_fl = World::found(&cfg_all, Arrangement::FreeList, profile).expect("fl");
    let mut w_c4 = World::found(&cfg_all, Arrangement::Compact, profile).expect("c4");
    w_c4.set_cpu_workers(NonZeroUsize::new(4).expect("4"))
        .expect("4 workers");

    let admission = w_c1.admit_memory(false, 0, 0).expect("admit_memory");
    assert!(admission.peak_bytes <= cfg_all.config().execution.maximum_total_bytes);
    let bound = w_c1.footprint().total();
    assert!(w_c1.allocated().total() <= bound);

    let mut max_rel = 0.0_f64;
    for tick in 0..20 {
        let Stepped::Published(r1) = w_c1.step().expect("c1") else {
            panic!("pause");
        };
        let _ = w_fl.step().expect("fl");
        let _ = w_c4.step().expect("c4");
        assert_eq!(w_c1.digest(), w_fl.digest(), "C.7 mismatch at {tick}");
        assert_eq!(w_c1.digest(), w_c4.digest(), "1 vs 4 mismatch at {tick}");
        assert!(
            w_c1.allocated().total() <= bound,
            "INV-15 overshoot at tick {tick}"
        );
        max_rel = max_rel.max(r1.closure.residual().abs() / r1.closure.scale().max(1.0));
    }
    check_inv03(&w_c1, profile, max_rel);

    // C.8 cancel rollback.
    let before = w_c1.digest();
    match w_c1.advance().expect("advance") {
        Advance::Ready(cand) => cand.cancel(),
        Advance::Paused(_) => panic!("pause"),
    }
    assert_eq!(w_c1.digest(), before, "C.8 cancel changed digest");

    // Checkpoint v3 round-trip.
    let mut writer = CheckpointWriter::new(cfg_all.config().model.as_str());
    w_c1.write_checkpoint(&mut writer).expect("write");
    let bytes = writer.finish().expect("finish");
    let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("parse");
    let mut restored = World::read_checkpoint(&reader, profile).expect("read");
    assert_eq!(restored.digest(), w_c1.digest());
    for tick in 0..8 {
        let _ = w_c1.step().expect("c1");
        let _ = restored.step().expect("restored");
        assert_eq!(
            restored.digest(),
            w_c1.digest(),
            "post-checkpoint mismatch at tick {tick}"
        );
    }
}

#[test]
fn e2e_trophallaxis_sustains_barren_central_nest() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let overrides: &[(&str, &str, &str)] = &[
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "64"),
        ("world", "grid_height", "64"),
        ("world", "tick_numerator", "1"),
        ("world", "tick_denominator", "32"),
        ("world", "diffusion", "0.02"),
        ("world", "decay_per_second", "0.15"),
        ("population", "initial_count", "32"),
        ("population", "maximum_count", "512"),
        ("population", "initial_energy", "3.0"),
        (
            "population",
            "capacity_policy",
            "\"refuse-excess-births-v1\"",
        ),
        ("sources", "count", "1"),
        ("sources", "power_per_source", "1e-9"),
        ("sources", "sigma_world_units", "1.0"),
        ("chemistry", "uptake_capacity_per_second", "4.0"),
        ("costs", "basal_energy_per_second", "0.10"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.2",
        ),
        ("reproduction", "cooldown_seconds", "0.25"),
        ("reproduction", "maximum_attempt_rate_per_second", "8.0"),
        ("controller", "hidden_widths", "[8]"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ];
    let toml = ecology_toml(
        "field-ecology-v2",
        overrides,
        r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.55
adhesion_range = 2.0
sweeps = 2

[v2]
profile = "minimal"

[v2.trophallaxis]
enabled = true
conductance_per_second = 6.0
transfer_efficiency = 0.95
reserve_energy = 0.25
maximum_export_fraction = 0.25
kin_cosine_exponent = 4
kin_threshold = 0.5
"#,
    );
    let cfg_on = ValidatedConfig::from_toml(&toml).expect("on");
    let cfg_off = ValidatedConfig::from_toml_with_overrides(
        &toml,
        &[FeatureOverride::Disable(
            V2Feature::Trophallaxis.slug().to_owned(),
        )],
    )
    .expect("off");

    let mut w_on = World::found(&cfg_on, Arrangement::Compact, profile).expect("w_on");
    let mut w_off = World::found(&cfg_off, Arrangement::Compact, profile).expect("w_off");

    // Seed founders on the outer food ring (`[35, 32]`) on tick 0 so the ring
    // feeds a real clonal lineage via chemically matched `InjectMaterial`
    // (neutral `InjectEnergy` yields negative efficiency at `b = 0.15`!) while
    // the central disk around `(16.0, 16.0)` stays completely barren.
    let ring_cells: [[u32; 2]; 8] = [
        [36, 32],
        [35, 35],
        [32, 36],
        [29, 35],
        [28, 32],
        [29, 29],
        [32, 28],
        [35, 29],
    ];
    let center = [16.0_f64, 16.0_f64];
    let barren_radius = 1.5_f64;

    let mut total_debited_on = 0.0_f64;
    let mut total_credited_on = 0.0_f64;
    let mut total_tax_on = 0.0_f64;
    let mut total_debited_off = 0.0_f64;
    let mut max_rel_on = 0.0_f64;
    let mut max_rel_off = 0.0_f64;
    let mut max_tr_balance_err = 0.0_f64;
    let mut interior_organism_ticks_on = 0_u64;
    let mut interior_organism_ticks_off = 0_u64;
    let mut max_depth_on = 0.0_f64;
    let mut food_dir = [0.0_f64; AFFINITY_TERMS];

    for tick in 0..1_024_u64 {
        let mut interventions = Vec::new();
        if tick == 0 {
            interventions.push(Intervention::IntroduceOrganisms {
                cell: [36, 32],
                count: 4,
                energy: 4.5,
            });
            interventions.push(Intervention::IntroduceOrganisms {
                cell: [32, 36],
                count: 4,
                energy: 4.5,
            });
        } else if tick == 1 {
            // Read the genetic affinity of the first ring-seeded founder so
            // ring `InjectMaterial` provides positive uptake efficiency (`η > 0`),
            // letting the clonal colony divide and bridge into the barren nest.
            let ring_founder = w_on
                .population()
                .ids()
                .last()
                .expect("introduced ring founder");
            food_dir = w_on
                .population()
                .genome(ring_founder)
                .expect("ring founder genome")
                .traits()
                .affinity()
                .map(|x| x * 0.99);
        }
        if tick >= 1 && tick % 2 == 0 {
            let cell = ring_cells[((tick / 2) as usize) % ring_cells.len()];
            interventions.push(Intervention::InjectMaterial {
                cell: [32, 36],
                amount: 8.0,
                direction: food_dir,
            });
            if cell != [32, 36] {
                interventions.push(Intervention::InjectMaterial {
                    cell,
                    amount: 8.0,
                    direction: food_dir,
                });
            }
        }
        let Stepped::Published(r_on) = w_on.step_with(&interventions).expect("on") else {
            panic!("pause on");
        };
        let Stepped::Published(r_off) = w_off.step_with(&interventions).expect("off") else {
            panic!("pause off");
        };
        let deb = r_on.closure.debited(Transfer::Trophallaxis);
        let cred = r_on.closure.credited(Transfer::Trophallaxis);
        let tax = r_on.closure.flow(Flow::TrophallaxisLoss);
        max_tr_balance_err = max_tr_balance_err.max(((deb - cred) - tax).abs());
        total_debited_on += deb;
        total_credited_on += cred;
        total_tax_on += tax;
        total_debited_off += r_off.closure.debited(Transfer::Trophallaxis);
        max_rel_on = max_rel_on.max(r_on.closure.residual().abs() / r_on.closure.scale().max(1.0));
        max_rel_off =
            max_rel_off.max(r_off.closure.residual().abs() / r_off.closure.scale().max(1.0));
        for org in w_on.population().iter() {
            assert!(org.energy.is_finite() && org.energy > 0.0);
            let r = (org.position[0] - center[0]).hypot(org.position[1] - center[1]);
            if r < barren_radius {
                interior_organism_ticks_on += 1;
                max_depth_on = max_depth_on.max(barren_radius - r);
            }
        }
        for org in w_off.population().iter() {
            let r = (org.position[0] - center[0]).hypot(org.position[1] - center[1]);
            if r < barren_radius {
                interior_organism_ticks_off += 1;
            }
        }
    }

    eprintln!(
        "[v2-13] e2e_trophallaxis_barren_nest (1024 ticks, 64x64): debited={total_debited_on:.6}, credited={total_credited_on:.6}, tax={total_tax_on:.6}, max_tr_bal_err={max_tr_balance_err:e}, max_rel_on={max_rel_on:e}, max_rel_off={max_rel_off:e}, interior_ticks_on={interior_organism_ticks_on}, interior_ticks_off={interior_organism_ticks_off}, max_interior_depth={max_depth_on:.3}"
    );
    check_inv03(&w_on, profile, max_rel_on);
    check_inv03(&w_off, profile, max_rel_off);
    assert!(
        max_tr_balance_err <= 1e-15,
        "exact-quantum trophallaxis per-tick balance error {max_tr_balance_err:e} exceeded 1e-15"
    );
    assert!(
        total_debited_on > 0.0 && total_credited_on > 0.0 && total_tax_on > 0.0,
        "expected active trophallaxis transfer and tax across 1024 ticks"
    );
    assert!(
        (total_debited_on - (total_credited_on + total_tax_on)).abs() <= 1e-11,
        "cumulative debited ({total_debited_on}) != credited ({total_credited_on}) + tax ({total_tax_on})"
    );
    assert_eq!(
        total_debited_off, 0.0,
        "--disable trophallaxis must record 0.0 transfer"
    );
    assert_ne!(w_on.digest(), w_off.digest());
}

#[test]
fn e2e_velocity_alignment_with_so2_propulsion_raises_polar_order() {
    // 1. Controlled clonal moving swarm in Sweep 1: prove alignment raises
    //    local polar order `Φ_local(4 D)` strongly while strictly obeying `TK-04`.
    let grid = FieldGrid::new(32, 32, 16.0, 16.0).expect("grid");
    let rules = test_contact_rules();
    let aff = unit_aff_channel(0);
    let n = 36_usize;
    let mut swarm_pos = Vec::with_capacity(n);
    let mut swarm_vel = Vec::with_capacity(n);
    for row in 0..6 {
        for col in 0..6 {
            let idx = row * 6 + col;
            swarm_pos.push([6.0 + (col as f64) * 0.55, 6.0 + (row as f64) * 0.55]);
            // Semi-aligned noisy headings in [-pi/2, +pi/2] around +x.
            let angle = (((idx * 37) % 100) as f64 * 0.01 - 0.5) * core::f64::consts::PI;
            let speed = 0.8 + ((idx % 5) as f64) * 0.1;
            swarm_vel.push([speed * angle.cos(), speed * angle.sin()]);
        }
    }
    let ids: Vec<u64> = (1..=(n as u64)).collect();
    let alive = vec![true; n];
    let affinities = vec![aff; n];
    let polar_order_of = |vels: &[[f64; 2]]| -> f64 {
        let mut sx = 0.0_f64;
        let mut sy = 0.0_f64;
        let mut count = 0_usize;
        for v in vels {
            let s = v[0].hypot(v[1]);
            if s > 1e-9 {
                sx += v[0] / s;
                sy += v[1] / s;
                count += 1;
            }
        }
        if count == 0 {
            0.0
        } else {
            sx.hypot(sy) / (count as f64)
        }
    };
    let phi_before = polar_order_of(&swarm_vel);
    let mut scratch = ContactScratch::default();
    let mut disp = Vec::new();
    let mut next_vel = Vec::new();
    for _ in 0..24 {
        relax_sweep1_with_context(
            &grid,
            &rules,
            Bodies {
                positions: &swarm_pos,
                ids: &ids,
                alive: &alive,
                affinities: Some(&affinities),
            },
            &Sweep1Context {
                velocity_alignment: Some(VelocityAlignmentSweepParams { strength: 0.35 }),
                velocities: Some(&swarm_vel),
                ..Sweep1Context::default()
            },
            Sweep1Output {
                displacements: &mut disp,
                next_polarities: None,
                neighbor_h0: None,
                trophallaxis_delta_quanta: None,
                trophallaxis_summary: None,
                next_velocities: Some(&mut next_vel),
            },
            &mut scratch,
            1,
        )
        .expect("swarm align");
        for i in 0..n {
            let sq0 = swarm_vel[i][0] * swarm_vel[i][0] + swarm_vel[i][1] * swarm_vel[i][1];
            let sq1 = next_vel[i][0] * next_vel[i][0] + next_vel[i][1] * next_vel[i][1];
            assert!(sq1 <= sq0, "TK-04 violated on swarm body {i}");
        }
        swarm_vel.clone_from(&next_vel);
    }
    let phi_after = polar_order_of(&swarm_vel);
    assert!(
        phi_after > phi_before + 0.15 && phi_after > 0.90,
        "velocity alignment must raise clonal swarm polar order strongly: before={phi_before:.4}, after={phi_after:.4}"
    );

    // 2. Full 512-tick E2E world pairing `V2-05` (`so2-motor`) + `velocity-alignment`
    //    vs `--disable velocity-alignment`, logging `Φ_local(4 D)` and `INV-03`.
    let profile = NumericalProfile::reference_f32().expect("profile");
    let toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 2.0
sweeps = 2

[v2]
profile = "minimal"

[v2.motor]
enabled = true
energy_gradient_gain = 2.0
chemical_gradient_gain = 2.0
chiral_contour_gain = 0.5
heading_propulsion_fraction = 0.6

[v2.contact.velocity_alignment]
enabled = true
strength = 0.35
"#,
    );
    let cfg_on = ValidatedConfig::from_toml(&toml).expect("on");
    let cfg_off = ValidatedConfig::from_toml_with_overrides(
        &toml,
        &[FeatureOverride::Disable(
            V2Feature::VelocityAlignment.slug().to_owned(),
        )],
    )
    .expect("off");

    let mut w_on = World::found(&cfg_on, Arrangement::Compact, profile).expect("w_on");
    let mut w_off = World::found(&cfg_off, Arrangement::Compact, profile).expect("w_off");
    let radius_4d = 4.0 * 0.5;

    let mut max_rel_on = 0.0_f64;
    let mut max_rel_off = 0.0_f64;
    let mut phi_sum_on = 0.0_f64;
    let mut phi_samples_on = 0_usize;
    let mut phi_sum_off = 0.0_f64;
    let mut phi_samples_off = 0_usize;
    for _tick in 0..512_u64 {
        let Stepped::Published(r_on) = w_on.step().expect("on") else {
            panic!("pause on");
        };
        let Stepped::Published(r_off) = w_off.step().expect("off") else {
            panic!("pause off");
        };
        max_rel_on = max_rel_on.max(r_on.closure.residual().abs() / r_on.closure.scale().max(1.0));
        max_rel_off =
            max_rel_off.max(r_off.closure.residual().abs() / r_off.closure.scale().max(1.0));
        let (phi_on, n_on) = local_polar_order(&w_on, radius_4d);
        if n_on > 0 {
            phi_sum_on += phi_on;
            phi_samples_on += 1;
        }
        let (phi_off, n_off) = local_polar_order(&w_off, radius_4d);
        if n_off > 0 {
            phi_sum_off += phi_off;
            phi_samples_off += 1;
        }
    }
    let mean_phi_on = if phi_samples_on > 0 {
        phi_sum_on / (phi_samples_on as f64)
    } else {
        0.0
    };
    let mean_phi_off = if phi_samples_off > 0 {
        phi_sum_off / (phi_samples_off as f64)
    } else {
        0.0
    };
    eprintln!(
        "[v2-13] e2e_velocity_alignment (512 ticks, V2-05+align): swarm_phi={phi_before:.4}->{phi_after:.4}, world_phi_local_4d_on={mean_phi_on:.4} (samples={phi_samples_on}), world_phi_local_4d_off={mean_phi_off:.4} (samples={phi_samples_off}), max_rel_on={max_rel_on:e}"
    );
    check_inv03(&w_on, profile, max_rel_on);
    check_inv03(&w_off, profile, max_rel_off);
    assert!(
        phi_samples_on > 0 && mean_phi_on > 0.0,
        "expected moving neighbors within 4D in V2-05 + velocity-alignment run"
    );
    assert_ne!(
        w_on.digest(),
        w_off.digest(),
        "velocity-alignment must measurably alter trajectories vs --disable velocity-alignment"
    );
}
