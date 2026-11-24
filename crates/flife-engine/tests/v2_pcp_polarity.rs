//! Integration, known-answer, stability, invariant, and E2E verification for
//! `V2-08` (`pcp-polarity` C003 and `emt-gate`) inside `flife-engine`.
//!
//! Covers:
//! - `pcp_wedge_newton_symmetry_and_apical_collapse_zero`: parallel lateral
//!   (`1.0`), stacked (`0.0`), 30°/60° off-sheet, head-to-head anti-parallel
//!   apical at `β = 0.4` (exact `0.0`), splayed hollow-cyst ring (`sin δ = β`
//!   -> `1.0` & zero torque), and exact Newton 3rd-law symmetry (`Δ_0 == -Δ_1`);
//! - `pcp_disabled_matches_isotropic_contact_bit_exact`: `--disable pcp-polarity`
//!   matches `field-ecology-v1-contact` bit-for-bit with `0 B` overhead;
//! - `pcp_torque_nonvanishing_at_d_eq_D_clamped_and_deterministic`: nonzero
//!   torque at touching `d == D`, `±0.25 rad` clamp on dense 20-neighbor stack
//!   (positive & negative `dtheta` through `normalize_turns`), frozen-polarity
//!   control (`μ_θ = 0`), and `1 vs 4` worker bit-exactness on 5,000 bodies;
//! - `pcp_synthetic_hollow_cyst_stability`: 16-cell circular cyst with
//!   `β = sin(π/16)` stays hollow, unstacked, and ~2-coordinated over 200 steps;
//! - `pcp_oriented_lateral_mitosis_and_emt_gate`: `α_pol = 1` lateral daughter
//!   placement at `1.05 D` along `±p^⊥`, polarity inheritance, and `emt-gate`
//!   `2 σ(o_adh)` scaling;
//! - `e2e_pcp_plus_hippo_forms_unstacked_sheets_and_resumes`: full world run
//!   with `pcp-polarity + hippo-crowding + emt-gate`, verifying live
//!   `World::polarities()`, `C.7`, `C.8`, `1 vs 4` workers, `IntroduceOrganisms`,
//!   `crop`/`graft` `Traits::reseal`, checkpoint v3 round-trip, `admit_memory`,
//!   and §21.4 `INV-03`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::num::NonZeroUsize;

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::clock::Tick;
use flife_core::ids::OrganismId;
use flife_core::rng::SemanticRng;
use flife_core::{FeatureOverride, V2Feature, ValidatedConfig};
use flife_engine::contact::Bodies;
use flife_engine::{
    Arrangement, ContactRules, ContactScratch, Intervention, MAX_POLARITY_DTHETA_RAD,
    NumericalProfile, PcpSweepParams, Reproduction, Stepped, Sweep1Context, Sweep1Output,
    WORLD_CHECKPOINT_CHUNKS, World, pcp_lateral_wedge_weight, pcp_pair_torque_term,
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

fn unit_aff_e0() -> [f32; AFFINITY_TERMS] {
    let mut a = [0.0_f64; AFFINITY_TERMS];
    a[0] = 1.0;
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

#[test]
fn pcp_wedge_newton_symmetry_and_apical_collapse_zero() {
    // 1. Parallel lateral: p_i = p_j = [0, 1], r_hat = [1, 0], beta = 0 => 1.0.
    let w_lat = pcp_lateral_wedge_weight([0.0, 1.0], [0.0, 1.0], [1.0, 0.0], 1.0, 0.0);
    assert!(
        (w_lat - 1.0).abs() <= 1e-15,
        "parallel lateral must be 1.0, got {w_lat}"
    );

    // 2. Stacked apical-basal: p_i = p_j = [0, 1], r_hat = [0, 1], beta = 0 => 0.0.
    let w_stack = pcp_lateral_wedge_weight([0.0, 1.0], [0.0, 1.0], [0.0, 1.0], 1.0, 0.0);
    assert_eq!(w_stack, 0.0, "stacked must have 0.0 adhesion");

    // 3. 30° and 60° off-sheet hex angles: cos^2(30°) = 0.75, cos^2(60°) = 0.25.
    let c30 = 3.0_f64.sqrt() * 0.5;
    let w_30 = pcp_lateral_wedge_weight([0.0, 1.0], [0.0, 1.0], [c30, 0.5], 1.0, 0.0);
    assert!(
        (w_30 - 0.75).abs() <= 1e-14,
        "30° off-sheet expected 0.75, got {w_30}"
    );
    let w_60 = pcp_lateral_wedge_weight([0.0, 1.0], [0.0, 1.0], [0.5, c30], 1.0, 0.0);
    assert!(
        (w_60 - 0.25).abs() <= 1e-14,
        "60° off-sheet expected 0.25, got {w_60}"
    );

    // 4. Anti-parallel apical head-to-head even at maximum beta = 0.4 => EXACT 0.0!
    let w_apical_anti = pcp_lateral_wedge_weight([0.0, 1.0], [0.0, -1.0], [0.0, 1.0], 1.0, 0.4);
    assert_eq!(
        w_apical_anti, 0.0,
        "anti-parallel head-to-head apical collapse must be exact 0.0 even at beta=0.4"
    );
    let w_apical_anti_lat = pcp_lateral_wedge_weight([0.0, 1.0], [0.0, -1.0], [1.0, 0.0], 1.0, 0.4);
    assert_eq!(
        w_apical_anti_lat, 0.0,
        "anti-parallel p_i . p_j = -1 must have G_apical == 0.0"
    );

    // 5. Splayed hollow-cyst ring: sin(delta) = beta = 0.25, inward apical polarity => 1.0 & zero torque.
    let beta = 0.25_f64;
    let sin_d = beta;
    let cos_d = (1.0 - sin_d * sin_d).sqrt();
    let p_i = [sin_d, -cos_d];
    let p_j = [-sin_d, -cos_d];
    let r_hat = [1.0, 0.0];
    let w_ring = pcp_lateral_wedge_weight(p_i, p_j, r_hat, 1.0, beta);
    assert!(
        (w_ring - 1.0).abs() <= 1e-14,
        "splayed ring at sin(delta)=beta must achieve 1.0, got {w_ring}"
    );
    let tau_ring = pcp_pair_torque_term(p_i, p_j, r_hat, beta);
    assert!(
        tau_ring.abs() <= 1e-14,
        "splayed ring at sin(delta)=beta must have zero equilibrium torque, got {tau_ring:e}"
    );

    // 6. Full Sweep-1 Newton 3rd-law symmetry (Delta_0 == -Delta_1 to <= 1e-15)
    // across asymmetric angles, alpha, beta, and emt_eff.
    let grid = FieldGrid::new(32, 32, 16.0, 16.0).expect("grid");
    let rules = test_contact_rules();
    let aff = unit_aff_e0();
    let cases = [
        (
            [4.0, 4.0],
            [4.75, 4.0],
            [0.0, 1.0],
            [0.0, 1.0],
            [1.0, 1.0],
            [0.0, 0.0],
            [1.0, 1.0],
        ),
        (
            [4.0, 4.0],
            [4.0, 4.75],
            [0.0, 1.0],
            [0.0, -1.0],
            [1.0, 1.0],
            [0.4, 0.4],
            [1.0, 1.0],
        ),
        (
            [4.0, 4.0],
            [4.6, 4.45],
            [0.6, 0.8],
            [-0.28, 0.96],
            [0.3, 0.9],
            [-0.2, 0.35],
            [0.4, 1.7],
        ),
        (
            [0.1, 15.9],
            [15.7, 0.3],
            [0.8, -0.6],
            [0.5, -0.8660254037844386],
            [0.8, 0.6],
            [0.1, 0.3],
            [1.9, 0.8],
        ),
    ];
    let mut scratch = ContactScratch::default();
    let mut displacements = Vec::new();
    let mut next_polarities = Vec::new();
    for (pos0, pos1, p0, p1, alphas, betas, emts) in cases {
        let positions = [pos0, pos1];
        let ids = [1, 2];
        let alive = [true, true];
        let affinities = [aff, aff];
        let polarities = [p0, p1];
        let gradients = [[0.0, 0.0], [0.0, 0.0]];
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
                pcp: Some(PcpSweepParams {
                    dt: 1.0 / 32.0,
                    torque_mobility: 2.0,
                    gradient_cue_strength: 0.0,
                }),
                polarities: Some(&polarities),
                polarity_strengths: Some(&alphas),
                wedge_curvatures: Some(&betas),
                energy_gradients: Some(&gradients),
                emt_factors: Some(&emts),
                cell_states_h0: None,
            },
            Sweep1Output {
                displacements: &mut displacements,
                next_polarities: Some(&mut next_polarities),
                neighbor_h0: None,
            },
            &mut scratch,
            1,
        )
        .expect("sweep1");
        assert!(
            (displacements[0][0] + displacements[1][0]).abs() <= 1e-15
                && (displacements[0][1] + displacements[1][1]).abs() <= 1e-15,
            "Newton 3rd law violated: d0={:?}, d1={:?}",
            displacements[0],
            displacements[1]
        );
    }
    // Head-to-head anti-parallel apical case (index 1) must have exact zero displacement.
    let positions = [[4.0, 4.0], [4.0, 4.75]];
    relax_sweep1_with_context(
        &grid,
        &rules,
        Bodies {
            positions: &positions,
            ids: &[1, 2],
            alive: &[true, true],
            affinities: Some(&[aff, aff]),
        },
        &Sweep1Context {
            pcp: Some(PcpSweepParams {
                dt: 1.0 / 32.0,
                torque_mobility: 2.0,
                gradient_cue_strength: 0.0,
            }),
            polarities: Some(&[[0.0, 1.0], [0.0, -1.0]]),
            polarity_strengths: Some(&[1.0, 1.0]),
            wedge_curvatures: Some(&[0.4, 0.4]),
            energy_gradients: Some(&[[0.0, 0.0], [0.0, 0.0]]),
            emt_factors: None,
            cell_states_h0: None,
        },
        Sweep1Output {
            displacements: &mut displacements,
            next_polarities: Some(&mut next_polarities),
            neighbor_h0: None,
        },
        &mut scratch,
        1,
    )
    .expect("apical zero");
    assert_eq!(displacements[0], [0.0, 0.0]);
    assert_eq!(displacements[1], [0.0, 0.0]);
}

#[test]
fn pcp_disabled_matches_isotropic_contact_bit_exact() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let contact_tail = r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 2.0
"#;
    let v1_toml = ecology_toml("field-ecology-v1-contact", small_overrides(), contact_tail);
    let v2_toml = ecology_toml(
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

[v2.polarity]
enabled = true
"#,
    );
    let v1_cfg = ValidatedConfig::from_toml(&v1_toml).expect("v1-contact");
    let v2_off_cfg = ValidatedConfig::from_toml_with_overrides(
        &v2_toml,
        &[FeatureOverride::Disable(
            V2Feature::PcpPolarity.slug().to_owned(),
        )],
    )
    .expect("v2 off");
    assert!(
        !v2_off_cfg
            .config()
            .v2
            .as_ref()
            .expect("v2")
            .is_enabled(V2Feature::PcpPolarity)
    );

    let mut w_v1 = World::found(&v1_cfg, Arrangement::Compact, profile).expect("v1 world");
    let mut w_off = World::found(&v2_off_cfg, Arrangement::Compact, profile).expect("v2 off");
    assert!(!w_off.tracks_polarity());
    assert!(w_off.polarities().is_none());
    assert_eq!(
        w_off.footprint().state,
        w_v1.footprint().state,
        "disabled pcp-polarity must allocate 0 B extra state"
    );
    assert_eq!(
        w_off.footprint().scratch,
        w_v1.footprint().scratch,
        "disabled pcp-polarity must allocate 0 B extra scratch"
    );
    assert_eq!(w_off.digest(), w_v1.digest());

    for tick in 0..24 {
        let _ = w_v1.step().expect("v1 step");
        let _ = w_off.step().expect("v2 off step");
        assert_eq!(
            w_off.digest(),
            w_v1.digest(),
            "disabled pcp-polarity diverged from field-ecology-v1-contact at tick {tick}"
        );
    }
}

#[test]
fn pcp_torque_nonvanishing_at_d_eq_d_clamped_and_deterministic() {
    let grid = FieldGrid::new(64, 64, 32.0, 32.0).expect("grid");
    let rules = test_contact_rules();
    let aff = unit_aff_e0();

    // 1. Touching pair at EXACT d == D (where adhesion pull vanishes, (d - D) == 0):
    // torque W_ij = kin * (1 - D/(R*D)) = 0.5 > 0 MUST rotate misaligned polarities!
    let mut scratch = ContactScratch::default();
    let mut disp = Vec::new();
    let mut next_pol = Vec::new();
    let p0_init = [0.5_f64, 0.75_f64.sqrt()];
    let p1_init = [0.0, 1.0];
    relax_sweep1_with_context(
        &grid,
        &rules,
        Bodies {
            positions: &[[10.0, 10.0], [10.5, 10.0]],
            ids: &[1, 2],
            alive: &[true, true],
            affinities: Some(&[aff, aff]),
        },
        &Sweep1Context {
            pcp: Some(PcpSweepParams {
                dt: 0.1,
                torque_mobility: 2.0,
                gradient_cue_strength: 0.0,
            }),
            polarities: Some(&[p0_init, p1_init]),
            polarity_strengths: Some(&[1.0, 1.0]),
            wedge_curvatures: Some(&[0.0, 0.0]),
            energy_gradients: Some(&[[0.0, 0.0], [0.0, 0.0]]),
            emt_factors: None,
            cell_states_h0: None,
        },
        Sweep1Output {
            displacements: &mut disp,
            next_polarities: Some(&mut next_pol),
            neighbor_h0: None,
        },
        &mut scratch,
        1,
    )
    .expect("touching torque");
    assert_eq!(
        disp[0],
        [0.0, 0.0],
        "at d == D adhesion displacement must vanish"
    );
    assert_ne!(
        next_pol[0], p0_init,
        "torque at touching d == D must be strictly nonzero (W_ij = 1 - 1/R > 0)"
    );
    // p0 tilted clockwise from +y should rotate counterclockwise back toward +y (p0_x decreases).
    assert!(
        next_pol[0][0] < p0_init[0],
        "restoring torque must align p0 toward p1: {} -> {}",
        p0_init[0],
        next_pol[0][0]
    );
    let norm0 = next_pol[0][0].hypot(next_pol[0][1]);
    assert!((norm0 - 1.0).abs() <= 1e-14);

    // 2. Frozen-polarity control (torque_mobility == 0.0) leaves p bit-identical.
    relax_sweep1_with_context(
        &grid,
        &rules,
        Bodies {
            positions: &[[10.0, 10.0], [10.5, 10.0]],
            ids: &[1, 2],
            alive: &[true, true],
            affinities: Some(&[aff, aff]),
        },
        &Sweep1Context {
            pcp: Some(PcpSweepParams {
                dt: 0.1,
                torque_mobility: 0.0,
                gradient_cue_strength: 5.0,
            }),
            polarities: Some(&[p0_init, p1_init]),
            polarity_strengths: Some(&[1.0, 1.0]),
            wedge_curvatures: Some(&[0.3, 0.3]),
            energy_gradients: Some(&[[10.0, -5.0], [0.0, 0.0]]),
            emt_factors: None,
            cell_states_h0: None,
        },
        Sweep1Output {
            displacements: &mut disp,
            next_polarities: Some(&mut next_pol),
            neighbor_h0: None,
        },
        &mut scratch,
        1,
    )
    .expect("frozen polarity");
    assert_eq!(next_pol[0].map(f64::to_bits), p0_init.map(f64::to_bits));

    // 3. Dense 20-neighbor stack with huge mobility/cue: BOTH positive and negative
    // dtheta strictly clamped to <= 0.25 rad via normalize_turns, unit norm preserved.
    let mut positions = vec![[16.0, 16.0]];
    let mut polarities = vec![[1.0, 0.0]];
    let mut grads = vec![[0.0, -100.0]]; // strong negative torque cue on body 0
    for k in 0..20 {
        let angle = (k as f64) * core::f64::consts::TAU / 20.0;
        positions.push([16.0 + 0.35 * angle.cos(), 16.0 + 0.35 * angle.sin()]);
        let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
        polarities.push([0.0, sign]);
        grads.push([0.0, 100.0 * sign]);
    }
    let n = positions.len();
    let ids: Vec<u64> = (1..=(n as u64)).collect();
    let alive = vec![true; n];
    let affinities = vec![aff; n];
    let alphas = vec![1.0; n];
    let betas = vec![0.35; n];
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
            pcp: Some(PcpSweepParams {
                dt: 1.0,
                torque_mobility: 10.0,
                gradient_cue_strength: 10.0,
            }),
            polarities: Some(&polarities),
            polarity_strengths: Some(&alphas),
            wedge_curvatures: Some(&betas),
            energy_gradients: Some(&grads),
            emt_factors: None,
            cell_states_h0: None,
        },
        Sweep1Output {
            displacements: &mut disp,
            next_polarities: Some(&mut next_pol),
            neighbor_h0: None,
        },
        &mut scratch,
        1,
    )
    .expect("dense clamp");
    for (before, after) in polarities.iter().zip(&next_pol) {
        let norm = after[0].hypot(after[1]);
        assert!((norm - 1.0).abs() <= 1e-13, "unit norm violated: {norm}");
        let dot = (before[0] * after[0] + before[1] * after[1]).clamp(-1.0, 1.0);
        let dtheta = dot.acos();
        assert!(
            dtheta <= MAX_POLARITY_DTHETA_RAD + 1e-12,
            "angular step {dtheta} exceeded clamp {MAX_POLARITY_DTHETA_RAD}"
        );
    }

    // 4. 1 vs 4 workers bit-exact determinism over 5,000 bodies (> MINIMUM_BODIES_PER_WORKER).
    let count = 5_000_usize;
    let mut p_pos = Vec::with_capacity(count);
    let mut p_pol = Vec::with_capacity(count);
    let mut p_alpha = Vec::with_capacity(count);
    let mut p_beta = Vec::with_capacity(count);
    let mut p_grad = Vec::with_capacity(count);
    for i in 0..count {
        let x = ((i * 73) % 3199) as f64 * 0.01;
        let y = ((i * 197) % 3199) as f64 * 0.01;
        let ang = (i as f64) * 0.618_033_988_75 * core::f64::consts::TAU;
        p_pos.push([x, y]);
        p_pol.push([ang.cos(), ang.sin()]);
        p_alpha.push(((i % 11) as f64) * 0.1);
        p_beta.push((((i % 9) as f64) - 4.0) * 0.08);
        p_grad.push([(i % 5) as f64 - 2.0, ((i / 5) % 5) as f64 - 2.0]);
    }
    let p_ids: Vec<u64> = (1..=(count as u64)).collect();
    let p_alive = vec![true; count];
    let p_aff = vec![aff; count];
    let mut disp_1 = Vec::new();
    let mut pol_1 = Vec::new();
    let mut disp_4 = Vec::new();
    let mut pol_4 = Vec::new();
    let ctx = Sweep1Context {
        pcp: Some(PcpSweepParams {
            dt: 1.0 / 32.0,
            torque_mobility: 2.0,
            gradient_cue_strength: 0.25,
        }),
        polarities: Some(&p_pol),
        polarity_strengths: Some(&p_alpha),
        wedge_curvatures: Some(&p_beta),
        energy_gradients: Some(&p_grad),
        emt_factors: None,
        cell_states_h0: None,
    };
    let bodies = Bodies {
        positions: &p_pos,
        ids: &p_ids,
        alive: &p_alive,
        affinities: Some(&p_aff),
    };
    relax_sweep1_with_context(
        &grid,
        &rules,
        bodies,
        &ctx,
        Sweep1Output {
            displacements: &mut disp_1,
            next_polarities: Some(&mut pol_1),
            neighbor_h0: None,
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
            displacements: &mut disp_4,
            next_polarities: Some(&mut pol_4),
            neighbor_h0: None,
        },
        &mut scratch,
        4,
    )
    .expect("4 workers");
    for i in 0..count {
        assert_eq!(disp_1[i].map(f64::to_bits), disp_4[i].map(f64::to_bits));
        assert_eq!(pol_1[i].map(f64::to_bits), pol_4[i].map(f64::to_bits));
    }
}

#[test]
fn pcp_synthetic_hollow_cyst_stability() {
    let grid = FieldGrid::new(32, 32, 16.0, 16.0).expect("grid");
    // Use preset nearest-neighbor adhesion range `1.5 * D` (confluent-world):
    // for 16 cells on a touching circle, second-nearest chord is `1.9616 * D > 1.5 * D`,
    // keeping adhesion strictly nearest-neighbor along the epithelial ring.
    let rules = ContactRules {
        range: 1.5,
        ..test_contact_rules()
    };
    let d = rules.diameter; // 0.5
    let n = 16_usize;
    let half_step = core::f64::consts::PI / (n as f64);
    let beta = half_step.sin(); // ~0.19509 <= 0.4
    let r_lumen = d / (2.0 * beta); // exact touching polygon circumradius (~1.281)
    let center = [8.0, 8.0];
    let aff = unit_aff_e0();

    let mut positions = Vec::with_capacity(n);
    let mut polarities = Vec::with_capacity(n);
    for k in 0..n {
        let theta = (k as f64) * core::f64::consts::TAU / (n as f64);
        // Small deterministic radial + polarity perturbation to prove dynamic stability.
        let r_pert = r_lumen * (1.0 + 0.04 * ((k as f64) * 1.7).sin());
        positions.push([
            center[0] + r_pert * theta.cos(),
            center[1] + r_pert * theta.sin(),
        ]);
        let p_ang = theta + core::f64::consts::PI + 0.08 * ((k as f64) * 2.3).cos();
        polarities.push([p_ang.cos(), p_ang.sin()]);
    }
    let ids: Vec<u64> = (1..=(n as u64)).collect();
    let alive = vec![true; n];
    let affinities = vec![aff; n];
    let alphas = vec![1.0; n];
    let betas = vec![beta; n];
    let gradients = vec![[0.0, 0.0]; n];
    let mut scratch = ContactScratch::default();
    let mut disp = Vec::new();
    let mut next_pol = Vec::new();

    for _ in 0..200 {
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
                pcp: Some(PcpSweepParams {
                    dt: 1.0 / 32.0,
                    torque_mobility: 2.0,
                    gradient_cue_strength: 0.0,
                }),
                polarities: Some(&polarities),
                polarity_strengths: Some(&alphas),
                wedge_curvatures: Some(&betas),
                energy_gradients: Some(&gradients),
                emt_factors: None,
                cell_states_h0: None,
            },
            Sweep1Output {
                displacements: &mut disp,
                next_polarities: Some(&mut next_pol),
                neighbor_h0: None,
            },
            &mut scratch,
            1,
        )
        .expect("cyst relax");
        for i in 0..n {
            positions[i] = grid
                .wrap_position([positions[i][0] + disp[i][0], positions[i][1] + disp[i][1]])
                .expect("wrap");
            polarities[i] = next_pol[i];
        }
    }

    // Recompute cyst center of mass.
    let cx = positions.iter().map(|p| p[0]).sum::<f64>() / (n as f64);
    let cy = positions.iter().map(|p| p[1]).sum::<f64>() / (n as f64);
    let mut min_r = f64::INFINITY;
    let mut min_pair = f64::INFINITY;
    let mut coord_sum = 0_usize;
    for i in 0..n {
        let r = (positions[i][0] - cx).hypot(positions[i][1] - cy);
        min_r = min_r.min(r);
        let norm = polarities[i][0].hypot(polarities[i][1]);
        assert!((norm - 1.0).abs() <= 1e-13);
        for j in 0..n {
            if i == j {
                continue;
            }
            let dist = (positions[i][0] - positions[j][0]).hypot(positions[i][1] - positions[j][1]);
            min_pair = min_pair.min(dist);
            if dist <= 1.35 * d {
                coord_sum += 1;
            }
        }
    }
    let mean_coord = (coord_sum as f64) / (n as f64);
    assert!(
        min_r > 0.7 * r_lumen,
        "hollow cyst collapsed: min_r={min_r}, expected > {}",
        0.7 * r_lumen
    );
    assert!(
        min_pair >= 0.85 * d,
        "hollow cyst stacked: min_pair={min_pair}, expected >= {}",
        0.85 * d
    );
    assert!(
        (1.8..=2.2).contains(&mean_coord),
        "hollow cyst coordination {mean_coord} must stay near 2.0 (1-cell sheet)"
    );
}

#[test]
fn pcp_oriented_lateral_mitosis_and_emt_gate() {
    let rng = SemanticRng::new(42);
    let parent_pol = [0.6_f64, 0.8_f64];
    let p_perp = [-0.8_f64, 0.6_f64];
    let d = 0.5_f64;

    // 1. Oriented lateral mitosis at alpha_pol = 1.0 places daughters at ~1.05 D along ±p^⊥.
    let mut saw_plus = false;
    let mut saw_minus = false;
    let mut lateral_align_sum = 0.0_f64;
    for t in 1..=64_u64 {
        let offset = Reproduction::lateral_mitosis_offset(
            &rng,
            OrganismId::new(7),
            Tick::new(t),
            parent_pol,
            d,
            1.0,
        )
        .expect("lateral offset");
        let dist = offset[0].hypot(offset[1]);
        assert!(
            (dist - 1.05 * d).abs() <= 1e-12,
            "lateral mitosis distance {dist} != {}",
            1.05 * d
        );
        let proj = (offset[0] * p_perp[0] + offset[1] * p_perp[1]) / dist;
        if proj > 0.0 {
            saw_plus = true;
        } else {
            saw_minus = true;
        }
        lateral_align_sum += proj.abs();

        // Inherited polarity remains unit-length and close to parent polarity.
        let child_pol =
            Reproduction::inherit_polarity(&rng, OrganismId::new(7), Tick::new(t), parent_pol)
                .expect("inherit pol");
        assert!((child_pol[0].hypot(child_pol[1]) - 1.0).abs() <= 1e-13);
        let dot = child_pol[0] * parent_pol[0] + child_pol[1] * parent_pol[1];
        assert!(
            dot > 0.95,
            "inherited polarity drifted too far from parent: dot={dot}"
        );
    }
    assert!(
        saw_plus && saw_minus,
        "lateral mitosis must choose both +p^⊥ and -p^⊥ sides"
    );
    let mean_align = lateral_align_sum / 64.0;
    assert!(
        mean_align > 0.85,
        "alpha_pol=1.0 lateral mitosis mean |cos| = {mean_align} must exceed 0.85"
    );

    // 2. EMT gate adhesion multiplier scales pair adhesion linearly (0x : 1x : 2x).
    let grid = FieldGrid::new(32, 32, 16.0, 16.0).expect("grid");
    let rules = test_contact_rules();
    let aff = unit_aff_e0();
    let mut scratch = ContactScratch::default();
    let mut run_emt = |emt: f64| -> f64 {
        let mut disp = Vec::new();
        relax_sweep1_with_context(
            &grid,
            &rules,
            Bodies {
                positions: &[[4.0, 4.0], [4.75, 4.0]],
                ids: &[1, 2],
                alive: &[true, true],
                affinities: Some(&[aff, aff]),
            },
            &Sweep1Context {
                emt_factors: Some(&[emt, emt]),
                ..Sweep1Context::default()
            },
            Sweep1Output {
                displacements: &mut disp,
                next_polarities: None,
                neighbor_h0: None,
            },
            &mut scratch,
            1,
        )
        .expect("emt relax");
        disp[0][0]
    };
    let d0 = run_emt(0.0);
    let d1 = run_emt(1.0);
    let d2 = run_emt(2.0);
    assert_eq!(d0, 0.0, "emt_eff=0 must detach adhesion completely");
    assert!(d1 > 0.0, "emt_eff=1 must pull toward neighbor");
    assert!(
        (d2 - 2.0 * d1).abs() <= 1e-14,
        "emt_eff=2 must double adhesion pull: {d2} vs 2*{d1}"
    );
}

#[test]
fn e2e_pcp_plus_hippo_forms_unstacked_sheets_and_resumes() {
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
sweeps = 3

[v2]
profile = "minimal"

[v2.crowding]
enabled = true
arrest_threshold = 4.0

[v2.polarity]
enabled = true
torque_mobility = 2.0
gradient_cue_strength = 0.25
initial_polarity_strength_max = 0.8
maximum_wedge_curvature = 0.25
polarity_mutation_std = 0.05
oriented_lateral_mitosis = true
emt_gate_enabled = true
"#,
    );
    let cfg = ValidatedConfig::from_toml(&toml).expect("pcp+hippo+emt cfg");
    assert_eq!(
        cfg.v2_abi().action_width,
        37,
        "emt-gate must append 1 action logit (36 + 1 = 37)"
    );

    let mut w_c1 = World::found(&cfg, Arrangement::Compact, profile).expect("c1");
    let mut w_fl = World::found(&cfg, Arrangement::FreeList, profile).expect("fl");
    let mut w_c4 = World::found(&cfg, Arrangement::Compact, profile).expect("c4");
    w_c4.set_cpu_workers(NonZeroUsize::new(4).expect("4"))
        .expect("4 workers");

    assert!(w_c1.tracks_polarity());
    let initial_pols = w_c1.polarities().expect("live polarities");
    assert_eq!(initial_pols.len(), w_c1.population().len());
    for p in &initial_pols {
        assert!((p[0].hypot(p[1]) - 1.0).abs() <= 1e-13);
    }

    // Memory admission (INV-15) must pass and bound allocated bytes.
    let admission = w_c1.admit_memory(false, 0, 0).expect("admit_memory");
    assert!(admission.peak_bytes <= cfg.config().execution.maximum_total_bytes);
    let bound = w_c1.footprint().total();
    assert!(w_c1.allocated().total() <= bound);

    // Step 32 ticks with an organism introduction on tick 4 to verify intervention::introduce.
    let mut max_rel = 0.0_f64;
    for tick in 0..32 {
        let interventions = if tick == 4 {
            vec![Intervention::IntroduceOrganisms {
                cell: [8, 8],
                count: 4,
                energy: 2.0,
            }]
        } else {
            Vec::new()
        };
        let Stepped::Published(r1) = w_c1.step_with(&interventions).expect("c1") else {
            panic!("pause");
        };
        let _ = w_fl.step_with(&interventions).expect("fl");
        let _ = w_c4.step_with(&interventions).expect("c4");
        assert_eq!(w_c1.digest(), w_fl.digest(), "C.7 mismatch at tick {tick}");
        assert_eq!(
            w_c1.digest(),
            w_c4.digest(),
            "1 vs 4 workers mismatch at tick {tick}"
        );
        assert!(
            w_c1.allocated().total() <= bound,
            "INV-15 overshoot at tick {tick}"
        );
        max_rel = max_rel.max(r1.closure.residual().abs() / r1.closure.scale().max(1.0));
    }
    check_inv03(&w_c1, profile, max_rel);

    // Every live organism has unit polarity and PolarityTraits on its genome.
    let post_pols = w_c1.polarities().expect("post polarities");
    assert_eq!(post_pols.len(), w_c1.population().len());
    assert!(!post_pols.is_empty());
    let mut any_rotated = false;
    for (org, pol) in w_c1.population().iter().zip(&post_pols) {
        assert_eq!(org.polarity, *pol);
        assert!((pol[0].hypot(pol[1]) - 1.0).abs() <= 1e-13);
        let traits = w_c1.population().genome(org.id).expect("genome").traits();
        assert!(
            traits.polarity_traits().is_some(),
            "every organism must carry PolarityTraits"
        );
        if initial_pols.first().is_some_and(|p0| p0 != pol) {
            any_rotated = true;
        }
    }
    assert!(any_rotated, "expected diverse/evolving polarity vectors");

    // C.8 cancel-safety: cancel leaves digest and polarities bit-identical.
    let digest_before = w_c1.digest();
    let pols_before = w_c1.polarities().expect("before");
    match w_c1.advance().expect("advance") {
        flife_engine::Advance::Ready(candidate) => candidate.cancel(),
        flife_engine::Advance::Paused(_) => panic!("pause"),
    }
    assert_eq!(w_c1.digest(), digest_before, "C.8 cancel changed digest");
    assert_eq!(w_c1.polarities().expect("after"), pols_before);

    // Checkpoint v3 write -> read -> step 12 more ticks bit-exact!
    let mut writer = CheckpointWriter::new(cfg.config().model.as_str());
    w_c1.write_checkpoint(&mut writer)
        .expect("write checkpoint");
    let bytes = writer.finish().expect("finish");
    let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("parse");
    let mut restored = World::read_checkpoint(&reader, profile).expect("read checkpoint");
    assert_eq!(restored.digest(), w_c1.digest());
    assert_eq!(restored.polarities(), w_c1.polarities());

    for tick in 0..12 {
        let Stepped::Published(r1) = w_c1.step().expect("c1") else {
            panic!("pause");
        };
        let _ = w_fl.step().expect("fl");
        let _ = restored.step().expect("restored");
        assert_eq!(
            w_c1.digest(),
            w_fl.digest(),
            "post-cancel mismatch at {tick}"
        );
        assert_eq!(
            w_c1.digest(),
            restored.digest(),
            "post-checkpoint mismatch at {tick}"
        );
        max_rel = max_rel.max(r1.closure.residual().abs() / r1.closure.scale().max(1.0));
    }
    check_inv03(&w_c1, profile, max_rel);

    // Crop + Graft preserves organism polarity AND PolarityTraits via Traits::reseal!
    let (cropped, _measured) = w_c1.cropped([8.0, 8.0], 6.0).expect("crop");
    for org in cropped.population().iter() {
        let orig = w_c1.population().get(org.id).expect("orig");
        assert_eq!(org.polarity, orig.polarity);
        let cropped_pol_traits = cropped
            .population()
            .genome(org.id)
            .expect("cropped genome")
            .traits()
            .polarity_traits();
        let orig_pol_traits = w_c1
            .population()
            .genome(org.id)
            .expect("orig genome")
            .traits()
            .polarity_traits();
        assert_eq!(cropped_pol_traits, orig_pol_traits);
    }
    let mut host = World::found(&cfg, Arrangement::Compact, profile).expect("host");
    let _ = host
        .graft(&cropped, [8.0, 8.0], 6.0, [0, 0])
        .expect("graft");
    for org in host.population().iter() {
        assert!((org.polarity[0].hypot(org.polarity[1]) - 1.0).abs() <= 1e-13);
        assert!(
            host.population()
                .genome(org.id)
                .expect("g")
                .traits()
                .polarity_traits()
                .is_some()
        );
    }
}
