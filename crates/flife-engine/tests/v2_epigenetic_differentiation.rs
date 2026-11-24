//! `V2-12`: Epigenetic `FL-054` recurrent unlock, slow gate bias (`b_epi`),
//! `Q_met == J` clonal metabolic differentiation (`a_eff(h_0)`), EMT adhesion
//! switch (`m(h_1)`), epigenetic birth inheritance (`symmetric-epigenetic-v1` /
//! `asymmetric-stem-v1`), and juxtacrine Notch-Delta (`sensor-neighbor-mean-v1`
//! / `direct-lateral-inhibition-v1`) verification suite.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::num::NonZeroUsize;

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::clock::Tick;
use flife_core::ids::OrganismId;
use flife_core::numerics::{sigmoid, symplectic_rotate_channels};
use flife_core::rng::SemanticRng;
use flife_core::{CoreError, FeatureOverride, V2Feature, ValidatedConfig};
use flife_engine::contact::Bodies;
use flife_engine::{
    Advance, Arrangement, Chemistry, ContactRules, ContactScratch, DifferentiationRules,
    HeldOutput, Intervention, NumericalProfile, Reproduction, StateInheritanceMode, Stepped,
    Sweep1Context, Sweep1Output, TickReport, V2Rules, WORLD_CHECKPOINT_CHUNKS, World, composition,
    effective_affinity, relax_sweep1_with_context, symplectic_waste_direction,
};
use flife_field::FieldGrid;
use flife_field::state::CHANNELS;
use flife_genome::{AFFINITY_TERMS, TraitLimits, founder_traits_v2};
use flife_neural::{
    ControllerBackend, ControllerJob, ControllerScratch, RawOutputs, update_recurrent_state,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

fn ecology_toml(model: &str, overrides: &[(&str, &str, &str)], tail: &str) -> String {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut section = String::new();
    let mut out = String::new();
    for line in appendix[start..start + length].lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed[1..trimmed.len() - 1].to_owned();
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if section.is_empty() && trimmed.starts_with("model =") {
            out.push_str(&format!("model = \"{model}\"\n"));
            continue;
        }
        if let Some((key, _)) = trimmed.split_once('=') {
            let key = key.trim();
            if let Some(&(_, _, value)) = overrides
                .iter()
                .find(|&&(sec, k, _)| sec == section && k == key)
            {
                out.push_str(&format!("{key} = {value}\n"));
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(tail);
    out
}

fn small_overrides() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        ("world", "width", "16.0"),
        ("world", "height", "16.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "32"),
        ("population", "maximum_count", "256"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "120.0"),
        ("sources", "sigma_world_units", "2.5"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.4"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.0",
        ),
        ("reproduction", "maximum_attempt_rate_per_second", "8.0"),
        ("reproduction", "cooldown_seconds", "0.125"),
        ("controller", "hidden_widths", "[8]"),
        ("controller", "evaluation_interval_ticks", "2"),
        ("execution", "cpu_workers", "1"),
    ]
}

fn step_world(world: &mut World) -> TickReport {
    match world.step().expect("step") {
        Stepped::Published(report) => *report,
        Stepped::Paused(shortfall) => panic!("unexpected pause: {shortfall:?}"),
    }
}

fn test_contact_rules() -> ContactRules {
    ContactRules {
        diameter: 0.5,
        stiffness: 0.75,
        adhesion: 0.35,
        range: 2.0,
        sweeps: 1,
        inhibits: false,
        inhibition: 0.0,
        repulsion: 0.0,
    }
}

fn unit_aff_channel(channel: usize) -> [f32; AFFINITY_TERMS] {
    let mut a = [0.0_f32; AFFINITY_TERMS];
    a[channel] = 1.0;
    a
}

/// Reference-matching batched controller backend verifying dynamic recurrent
/// `(input_width, raw_output_width)` dispatch with zero alignment deferrals.
#[derive(Debug)]
struct DynamicReferenceBatchBackend {
    expected_input_width: usize,
    expected_raw_output_width: usize,
    scratch: ControllerScratch,
}

impl ControllerBackend for DynamicReferenceBatchBackend {
    fn describe(&self) -> String {
        format!(
            "dynamic-reference-batch({}/{})",
            self.expected_input_width, self.expected_raw_output_width
        )
    }

    fn resident_bytes(&self) -> u64 {
        0
    }

    fn forget(&mut self) {}

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<RawOutputs>>,
    ) -> Result<(), CoreError> {
        outputs.clear();
        outputs.reserve(jobs.len());
        for job in jobs {
            assert_eq!(
                job.inputs.len(),
                self.expected_input_width,
                "batched job input width mismatch"
            );
            assert_eq!(
                job.controller.topology().raw_output_width(),
                self.expected_raw_output_width,
                "batched job raw_output_width mismatch"
            );
            let raw = job.controller.evaluate_raw(job.inputs, &mut self.scratch)?;
            assert_eq!(raw.len(), self.expected_raw_output_width);
            outputs.push(Some(raw));
        }
        Ok(())
    }
}

#[test]
fn recurrent_epigenetic_gate_bias_and_asymmetric_birth() {
    // 1. Slow epigenetic gate bias `b_epi = 2.5` yields `g = sigmoid(-2.5) ≈ 0.07585818`
    //    at `z_g = 0.0`, vs `g = 0.5` at `b_epi = 0.0`.
    let state = [0.0_f32, 0.0_f32];
    let raw_tail = [1.0_f32, -1.0_f32, 0.0_f32, 0.0_f32];
    let mut next_slow = [0.0_f32; 2];
    let mut next_fast = [0.0_f32; 2];
    update_recurrent_state(&state, &raw_tail, 2.5, &mut next_slow).expect("slow gate");
    update_recurrent_state(&state, &raw_tail, 0.0, &mut next_fast).expect("fast gate");

    let g_expected = sigmoid(-2.5).expect("sigmoid(-2.5)") as f32;
    assert!(
        (f64::from(g_expected) - 0.075_858_18).abs() < 1e-6,
        "expected g ≈ 0.07585818, got {g_expected}"
    );
    let cand0 = 1.0_f32.tanh();
    assert!(
        (next_slow[0] - g_expected * cand0).abs() <= 1e-6,
        "next_slow[0]={} vs expected={}",
        next_slow[0],
        g_expected * cand0
    );
    assert!(
        next_slow[0].abs() < 0.2 * next_fast[0].abs(),
        "b_epi=2.5 must slow state drift relative to b_epi=0.0"
    );

    // 2. Epigenetic birth inheritance equations (`zero`, `symmetric-epigenetic-v1`,
    //    `asymmetric-stem-v1`) and `HeldOutput::newborn_with_state` (`C.13`).
    let rng = SemanticRng::new(42);
    let parent = OrganismId::new(7);
    let tick = Tick::new(19);
    let parent_state = [0.6_f32, -0.4_f32, 0.25_f32];

    let zero_rules = DifferentiationRules {
        state_width: 3,
        epigenetic_gate_bias: 2.5,
        state_inheritance: StateInheritanceMode::Zero,
        inheritance_retention: 0.8,
        inheritance_jitter: 0.05,
        stem_asymmetry: -0.8,
        affinity_rotation_enabled: true,
        emt_adhesion_enabled: true,
    };
    assert_eq!(
        Reproduction::inherit_epigenetic_state(&rng, parent, tick, &parent_state, zero_rules)
            .expect("zero"),
        None
    );

    // Symmetric inheritance with zero jitter matches exact `alpha * h_p`.
    let sym_no_jitter = DifferentiationRules {
        state_inheritance: StateInheritanceMode::SymmetricEpigenetic,
        inheritance_jitter: 0.0,
        ..zero_rules
    };
    let sym =
        Reproduction::inherit_epigenetic_state(&rng, parent, tick, &parent_state, sym_no_jitter)
            .expect("sym")
            .expect("some");
    assert_eq!(sym.as_slice().len(), 3);
    for (k, (&got, &hp)) in sym.as_slice().iter().zip(&parent_state).enumerate() {
        let expected = (0.8_f64 * f64::from(hp)) as f32;
        assert_eq!(
            got.to_bits(),
            expected.to_bits(),
            "symmetric no-jitter lane {k}"
        );
    }

    // Asymmetric stem division:
    //   k = 0: `c_stem = sigmoid(3.0 * h_p[0])`, `h_child[0] = clamp(alpha * h_p[0] + c_stem * beta, -1, 1)`
    //   k = 1..S: symmetric `(alpha * h_p[k] + sigma * xi_k).clamp(-1, 1)`
    let asym_rules = DifferentiationRules {
        state_inheritance: StateInheritanceMode::AsymmetricStem,
        inheritance_retention: 0.8,
        inheritance_jitter: 0.05,
        stem_asymmetry: -0.8,
        ..zero_rules
    };
    let asym_a =
        Reproduction::inherit_epigenetic_state(&rng, parent, tick, &parent_state, asym_rules)
            .expect("asym a")
            .expect("some");
    let asym_b =
        Reproduction::inherit_epigenetic_state(&rng, parent, tick, &parent_state, asym_rules)
            .expect("asym b")
            .expect("some");
    assert_eq!(
        asym_a, asym_b,
        "epigenetic inheritance must be semantically deterministic"
    );
    let c_stem = sigmoid(3.0 * f64::from(parent_state[0])).expect("c_stem");
    let expected_h0 = (0.8 * f64::from(parent_state[0]) + c_stem * (-0.8)).clamp(-1.0, 1.0) as f32;
    assert_eq!(
        asym_a.as_slice()[0].to_bits(),
        expected_h0.to_bits(),
        "asymmetric k=0 mismatch"
    );
    // Jittered k=1..S must remain bounded in [-1, 1] and differ from zero-jitter alpha * hp.
    for (k, (&val, &hp)) in asym_a
        .as_slice()
        .iter()
        .zip(&parent_state)
        .enumerate()
        .skip(1)
    {
        assert!(val.is_finite() && (-1.0_f32..=1.0_f32).contains(&val));
        assert_ne!(
            val.to_bits(),
            ((0.8 * f64::from(hp)) as f32).to_bits(),
            "nonzero jitter must perturb symmetric lane {k}"
        );
    }

    // Newborn `HeldOutput::newborn_with_state` keeps `evaluated_at == None` (`C.13`).
    let held = asym_a.into_held_output().expect("newborn held");
    assert!(!held.is_valid());
    assert_eq!(held.evaluated_at(), None);
    assert_eq!(held.outputs(), None);
    assert_eq!(held.recurrent_state(), asym_a.as_slice());
}

#[test]
fn a_eff_orthogonal_rotation_full_signed_range_left_fold_safe() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let single_toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[v2]
profile = "minimal"

[v2.differentiation]
enabled = true
state_width = 2
affinity_rotation_enabled = true
"#,
    );
    let dual_toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[v2]
profile = "minimal"

[v2.carriers]
enabled = true

[v2.differentiation]
enabled = true
state_width = 2
affinity_rotation_enabled = true
"#,
    );
    let single_cfg = ValidatedConfig::from_toml(&single_toml).expect("single cfg");
    let dual_cfg = ValidatedConfig::from_toml(&dual_toml).expect("dual cfg");
    let single_rules = V2Rules::from_config(&single_cfg).expect("single rules");
    let dual_rules = V2Rules::from_config(&dual_cfg).expect("dual rules");
    let norm_limit = single_cfg.config().chemistry.affinity_norm_limit;
    let limits = TraitLimits::from_config(&single_cfg).expect("limits");
    let rng = SemanticRng::new(77);

    // Build adversarial unit affinity vectors (uniform, sparse, alternating, founder-drawn).
    let mut test_traits = Vec::new();
    for id in 0..4_u64 {
        test_traits.push(
            founder_traits_v2(&rng, OrganismId::new(id), &limits, 1.0).expect("founder traits"),
        );
    }
    let mut uniform_dir = [0.0_f64; CHANNELS];
    let inv_sqrt_32 = 1.0 / (CHANNELS as f64).sqrt();
    for (k, slot) in uniform_dir.iter_mut().enumerate() {
        *slot = if k % 2 == 0 {
            inv_sqrt_32
        } else {
            -inv_sqrt_32
        };
    }
    test_traits.push(
        flife_genome::Traits::new(uniform_dir, [0.0; 33], 1.0, &limits).expect("uniform traits"),
    );

    // Sweep 101 grid points across [-1.0, 1.0] plus explicit negative edge cases.
    let mut h0_values: Vec<f32> = (0..=100)
        .map(|i| (-1.0_f64 + 2.0 * (i as f64) / 100.0) as f32)
        .collect();
    h0_values.extend_from_slice(&[-1.0_f32, -0.5, -1e-12, -0.0, 0.0, 1e-12, 0.5, 1.0]);

    for (label, rules, active_ch) in [
        ("single-32ch", &single_rules, 32_usize),
        ("dual-24ch", &dual_rules, 24_usize),
    ] {
        for traits in &test_traits {
            let a = *traits.affinity();
            let ja = symplectic_rotate_channels(&a, active_ch, norm_limit).expect("J a");

            // Exact endpoints: h_0 = 0, -0 -> a; h_0 = +1 -> J a; h_0 = -1 -> -J a.
            assert_eq!(
                effective_affinity(traits, 0.0, rules, norm_limit).expect("h0=0"),
                a,
                "{label}: a_eff(0) must equal a"
            );
            assert_eq!(
                effective_affinity(traits, -0.0, rules, norm_limit).expect("h0=-0"),
                a,
                "{label}: a_eff(-0) must equal a"
            );
            let at_pos1 = effective_affinity(traits, 1.0, rules, norm_limit).expect("h0=+1");
            let at_neg1 = effective_affinity(traits, -1.0, rules, norm_limit).expect("h0=-1");
            for k in 0..active_ch {
                assert!(
                    (at_pos1[k] - ja[k]).abs() <= 1e-14,
                    "{label}: a_eff(+1)[{k}]={} != ja[{k}]={}",
                    at_pos1[k],
                    ja[k]
                );
                assert!(
                    (at_neg1[k] + ja[k]).abs() <= 1e-14,
                    "{label}: a_eff(-1)[{k}]={} != -ja[{k}]={}",
                    at_neg1[k],
                    -ja[k]
                );
            }
            for k in active_ch..CHANNELS {
                assert_eq!(
                    at_pos1[k], a[k],
                    "{label}: trailing Carrier-1 channel {k} must remain untouched"
                );
                assert_eq!(
                    at_neg1[k], a[k],
                    "{label}: trailing Carrier-1 channel {k} must remain untouched"
                );
            }

            // Full sweep: left-fold norm <= norm_limit^2 and 2nd-law waste orthogonality.
            for &h0 in &h0_values {
                let a_eff = effective_affinity(traits, h0, rules, norm_limit).expect("a_eff");
                let mut left_fold_sq = 0.0_f64;
                for &component in &a_eff {
                    assert!(component.is_finite());
                    left_fold_sq += component * component;
                }
                assert!(
                    left_fold_sq <= norm_limit * norm_limit,
                    "{label} h0={h0}: left-fold ||a_eff||^2 = {left_fold_sq} > {}",
                    norm_limit * norm_limit
                );
                let waste =
                    symplectic_waste_direction(&a_eff, active_ch, norm_limit).expect("waste");
                let dot: f64 = a_eff.iter().zip(&waste).map(|(x, y)| x * y).sum();
                assert!(
                    dot.abs() <= 1e-13,
                    "{label} h0={h0}: a_eff · J a_eff = {dot:e} must vanish"
                );
            }
        }
    }

    // Verify `LocalChemistry` (`World::organism_snapshot`) and `World::effective_affinity`
    // evaluate efficiency with rotated `a_eff(h_0)` after tick 1!
    let mut world = World::found(&single_cfg, Arrangement::Compact, profile).expect("found");
    step_world(&mut world);
    let org = world
        .population()
        .iter()
        .find(|o| o.held.recurrent_state_array()[0] != 0.0)
        .expect("at least one organism has nonzero h_0 after tick 1");
    let genome = world.population().genome(org.id).expect("genome");
    let a_eff = world
        .effective_affinity(org.id)
        .expect("effective_affinity")
        .expect("alive");
    assert_ne!(
        a_eff,
        *genome.traits().affinity(),
        "nonzero h_0 must rotate effective_affinity away from genetic affinity"
    );
    let snap = world
        .organism_snapshot(org.id)
        .expect("snapshot")
        .expect("alive");
    let chemistry = Chemistry::from_config(&single_cfg).expect("chemistry");
    let aff = flife_engine::Affinity::new(a_eff, &chemistry).expect("affinity");
    for cell in &snap.local.cells {
        let z = composition(world.field(), cell.cell).expect("composition");
        let expected_eta = chemistry.efficiency(&aff, &z).expect("eta");
        assert_eq!(
            cell.efficiency.to_bits(),
            expected_eta.to_bits(),
            "LocalChemistry must evaluate efficiency on rotated a_eff(h_0)"
        );
    }
}

#[test]
fn emt_h1_adhesion_switch_and_post_phase4_timing() {
    // 1. `m(h_1) = clamp(1 + h_1, 0, 1)` modulates Sweep-1 adhesion cleanly:
    //    h_1 = +1 -> 1.0x (clamped), h_1 = 0 -> 1.0x, h_1 = -0.5 -> 0.5x, h_1 = -1.0 -> 0.0x.
    let grid = FieldGrid::new(32, 32, 16.0, 16.0).expect("grid");
    let rules = test_contact_rules();
    let aff = unit_aff_channel(0);
    let mut scratch = ContactScratch::default();
    let mut run_h1 = |h1: f32| -> f64 {
        let m = (1.0 + f64::from(h1)).clamp(0.0, 1.0);
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
                emt_factors: Some(&[m, m]),
                ..Sweep1Context::default()
            },
            Sweep1Output {
                displacements: &mut disp,
                next_polarities: None,
                neighbor_h0: None,
                trophallaxis_delta_quanta: None,
                trophallaxis_summary: None,
                next_velocities: None,
            },
            &mut scratch,
            1,
        )
        .expect("emt relax");
        disp[0][0]
    };
    let d_pos1 = run_h1(1.0);
    let d_zero = run_h1(0.0);
    let d_half = run_h1(-0.5);
    let d_neg1 = run_h1(-1.0);
    assert_eq!(d_pos1.to_bits(), d_zero.to_bits());
    assert!(d_zero > 0.0, "h_1=0 must preserve epithelial adhesion pull");
    assert!(
        (d_half - 0.5 * d_zero).abs() <= 1e-14,
        "h_1=-0.5 must halve adhesion pull: {d_half} vs {}",
        0.5 * d_zero
    );
    assert_eq!(
        d_neg1, 0.0,
        "h_1=-1.0 must detach mesenchymal adhesion completely"
    );

    // 2. Post-Phase-4 timing proof: on tick 1 when a single founder starts at
    //    `h_0 = 0.0`, Phase 4 updates `h_0` to nonzero `h'_0`, and Phase 6
    //    `symplectic-waste` deposits along `J a_eff(h'_0)` (NOT stale `J a_eff(0)`).
    let profile = NumericalProfile::reference_f32().expect("profile");
    let waste_toml = ecology_toml(
        "field-ecology-v2",
        &[
            ("world", "width", "16.0"),
            ("world", "height", "16.0"),
            ("world", "grid_width", "32"),
            ("world", "grid_height", "32"),
            ("population", "initial_count", "1"),
            ("population", "maximum_count", "16"),
            ("population", "initial_energy", "1.5"),
            ("sources", "count", "1"),
            ("sources", "power_per_source", "1e-9"),
            ("sources", "sigma_world_units", "1.0"),
            ("chemistry", "uptake_capacity_per_second", "16.0"),
            ("costs", "basal_energy_per_second", "0.1"),
            ("controller", "hidden_widths", "[8]"),
            ("controller", "evaluation_interval_ticks", "1"),
            ("execution", "cpu_workers", "1"),
        ],
        r#"
[v2]
profile = "minimal"

[v2.emission.surplus_floor]
enabled = true
energy_floor = 1.9

[v2.metabolism.waste]
enabled = true
byproduct_recycle_fraction = 0.5

[v2.differentiation]
enabled = true
state_width = 2
epigenetic_gate_bias = 0.0
affinity_rotation_enabled = true
emt_adhesion_enabled = true
"#,
    );
    let cfg = ValidatedConfig::from_toml(&waste_toml).expect("waste cfg");
    let mut world = World::found(&cfg, Arrangement::Compact, profile).expect("found");
    let founder = world.population().iter().next().expect("founder");
    // Inject neutral energy at founder's cell so uptake is nonzero, unconverted > 0,
    // and existing cell moment is 0 (so post-tick field moment comes purely from waste).
    let grid = *world.field().grid();
    let cx = ((founder.position[0] / grid.cell_width()).floor() as u32).min(grid.cells_x() - 1);
    let cy = ((founder.position[1] / grid.cell_height()).floor() as u32).min(grid.cells_y() - 1);
    let Advance::Ready(cand) = world
        .advance_with(&[Intervention::InjectEnergy {
            cell: [cx, cy],
            amount: 4.0,
        }])
        .expect("advance")
    else {
        panic!("unexpected pause");
    };
    cand.publish().expect("publish");

    let after = world
        .population()
        .get(founder.id)
        .expect("founder survives");
    let h0_post = after.held.recurrent_state_array()[0];
    assert_ne!(
        h0_post, 0.0,
        "founder must acquire nonzero post-Phase-4 h_0 on tick 1"
    );

    // Sum total field moment vector (purely from tick-1 symplectic waste deposit).
    let mut total_m = [0.0_f64; CHANNELS];
    let cell_count = grid.cell_count() as usize;
    for cell in 0..cell_count {
        let values = world.field().cell_at(cell).expect("cell");
        for (dst, &m) in total_m.iter_mut().zip(&values.moments) {
            *dst += f64::from(m);
        }
    }
    let m_norm = total_m.iter().map(|v| v * v).sum::<f64>().sqrt();
    assert!(
        m_norm > 1e-4,
        "symplectic waste must deposit nonzero field moment, got {m_norm:e}"
    );
    let m_unit: [f64; CHANNELS] = total_m.map(|v| v / m_norm);

    let a_post = world
        .effective_affinity(founder.id)
        .expect("a_eff")
        .expect("alive");
    let a_genetic = *world
        .population()
        .genome(founder.id)
        .expect("genome")
        .traits()
        .affinity();
    let dot_post: f64 = m_unit.iter().zip(&a_post).map(|(x, y)| x * y).sum();
    let g_norm = a_genetic.iter().map(|v| v * v).sum::<f64>().sqrt();
    let dot_stale_unit: f64 = m_unit
        .iter()
        .zip(&a_genetic)
        .map(|(x, y)| x * (y / g_norm))
        .sum();
    let expected_stale_dot = -(0.5 * core::f64::consts::PI * f64::from(h0_post)).sin();
    assert!(
        dot_post.abs() <= 1e-4,
        "waste must be orthogonal to POST-Phase-4 a_eff(h'_0), got dot={dot_post:e}"
    );
    assert!(
        (dot_stale_unit - expected_stale_dot).abs() <= 1e-3,
        "waste must match POST-Phase-4 J a_eff(h'_0) rotation (dot_stale_unit={dot_stale_unit:e}, expected={expected_stale_dot:e}, h'_0={h0_post})"
    );
}

#[test]
fn notch_delta_clonal_kin_not_broken_by_differentiation_and_checkpoints() {
    let grid = FieldGrid::new(32, 32, 16.0, 16.0).expect("grid");
    let rules = test_contact_rules();

    // 1. Clonal sisters with identical GENETIC affinity (a_0 = a_1) but
    //    differentiated states h_0 = +1 (a_eff = J a) and h_0 = 0 (a_eff = a)
    //    retain exact `kin_w == 1.0`, while an orthogonal non-kin neighbor
    //    contributes zero weight.
    let mut scratch = ContactScratch::default();
    let mut disp = Vec::new();
    let mut neighbor_h0 = Vec::new();
    let aff_kin = unit_aff_channel(0);
    let aff_other = unit_aff_channel(2);
    relax_sweep1_with_context(
        &grid,
        &rules,
        Bodies {
            positions: &[[4.0, 4.0], [4.75, 4.0], [4.0, 4.75]],
            ids: &[1, 2, 3],
            alive: &[true, true, true],
            affinities: Some(&[aff_kin, aff_kin, aff_other]),
        },
        &Sweep1Context {
            cell_states_h0: Some(&[1.0_f32, 0.0_f32, -1.0_f32]),
            ..Sweep1Context::default()
        },
        Sweep1Output {
            displacements: &mut disp,
            next_polarities: None,
            neighbor_h0: Some(&mut neighbor_h0),
            trophallaxis_delta_quanta: None,
            trophallaxis_summary: None,
            next_velocities: None,
        },
        &mut scratch,
        1,
    )
    .expect("clonal notch-delta");
    assert_eq!(
        neighbor_h0[0], 0.0_f32,
        "sister 0 sees only clonal sister 1 (h0=0.0), ignoring orthogonal non-kin 2 (h0=-1.0)"
    );
    assert!(
        (neighbor_h0[1] - 1.0_f32).abs() <= 1e-6,
        "sister 1 sees clonal sister 0 (h0=+1.0) with kin_w=1.0, got {}",
        neighbor_h0[1]
    );
    assert_eq!(
        neighbor_h0[2], 0.0_f32,
        "orthogonal non-kin 2 has zero kin neighbors -> h_bar == 0.0"
    );

    // 2. 1 vs 4 worker bit-exact determinism over 5,000 bodies.
    let count = 5_000_usize;
    let big_grid = FieldGrid::new(64, 64, 32.0, 32.0).expect("big grid");
    let mut pos = Vec::with_capacity(count);
    let mut h0s = Vec::with_capacity(count);
    let mut affs = Vec::with_capacity(count);
    for i in 0..count {
        pos.push([
            ((i * 73) % 3199) as f64 * 0.01,
            ((i * 41) % 3199) as f64 * 0.01,
        ]);
        h0s.push((((i % 21) as f32) - 10.0) * 0.1);
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
        cell_states_h0: Some(&h0s),
        ..Sweep1Context::default()
    };
    let (mut d1, mut n1, mut d4, mut n4) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    relax_sweep1_with_context(
        &big_grid,
        &rules,
        bodies,
        &ctx,
        Sweep1Output {
            displacements: &mut d1,
            next_polarities: None,
            neighbor_h0: Some(&mut n1),
            trophallaxis_delta_quanta: None,
            trophallaxis_summary: None,
            next_velocities: None,
        },
        &mut scratch,
        1,
    )
    .expect("1 worker");
    relax_sweep1_with_context(
        &big_grid,
        &rules,
        bodies,
        &ctx,
        Sweep1Output {
            displacements: &mut d4,
            next_polarities: None,
            neighbor_h0: Some(&mut n4),
            trophallaxis_delta_quanta: None,
            trophallaxis_summary: None,
            next_velocities: None,
        },
        &mut scratch,
        4,
    )
    .expect("4 workers");
    for i in 0..count {
        assert_eq!(d1[i].map(f64::to_bits), d4[i].map(f64::to_bits));
        assert_eq!(n1[i].to_bits(), n4[i].to_bits());
    }

    // 3. Uniform clonal chain under `direct-lateral-inhibition-v1` (eta = 0.25 <= 0.3):
    //    bifurcates into stable alternating tip-stalk states without 2-tick oscillation.
    let chain_rules = ContactRules {
        range: 1.6,
        ..rules
    };
    let n_chain = 8_usize;
    let chain_pos: Vec<[f64; 2]> = (0..n_chain)
        .map(|k| [2.0 + (k as f64) * 0.65, 8.0])
        .collect();
    let chain_ids: Vec<u64> = (1..=(n_chain as u64)).collect();
    let chain_alive = vec![true; n_chain];
    let chain_aff = vec![aff_kin; n_chain];
    let mut h_chain: Vec<f32> = (0..n_chain)
        .map(|k| if k % 2 == 0 { 0.05 } else { -0.05 })
        .collect();
    let mut h_prev_step = h_chain.clone();
    let eta = 0.25_f32;
    for _ in 0..40 {
        h_prev_step.clone_from(&h_chain);
        relax_sweep1_with_context(
            &grid,
            &chain_rules,
            Bodies {
                positions: &chain_pos,
                ids: &chain_ids,
                alive: &chain_alive,
                affinities: Some(&chain_aff),
            },
            &Sweep1Context {
                cell_states_h0: Some(&h_chain),
                ..Sweep1Context::default()
            },
            Sweep1Output {
                displacements: &mut disp,
                next_polarities: None,
                neighbor_h0: Some(&mut neighbor_h0),
                trophallaxis_delta_quanta: None,
                trophallaxis_summary: None,
                next_velocities: None,
            },
            &mut scratch,
            1,
        )
        .expect("chain sweep");
        for (h, &hbar) in h_chain.iter_mut().zip(&neighbor_h0) {
            let autonomous = (0.9 * *h + 0.1 * (2.0 * *h).tanh()).clamp(-1.0, 1.0);
            *h = (autonomous - eta * hbar).clamp(-1.0, 1.0);
        }
    }
    for k in 0..n_chain {
        assert!(h_chain[k].abs() > 0.5, "chain cell {k} must differentiate");
        assert_eq!(
            h_chain[k].signum(),
            h_prev_step[k].signum(),
            "eta=0.25 must NOT cause 2-tick checkerboard sign-flip oscillation at cell {k}"
        );
        if k + 1 < n_chain {
            assert!(
                h_chain[k] * h_chain[k + 1] < 0.0,
                "adjacent clonal cells {k} and {} must alternate signs: {}, {}",
                k + 1,
                h_chain[k],
                h_chain[k + 1]
            );
        }
    }

    // 4. Live World checkpoint v3 round-trip (preserving newborn `evaluated_at == None`
    //    with nonzero inherited `h_child` AND nonzero `last_neighbor_h0`) + watched
    //    `organism_snapshot` activation parity in BOTH Notch-Delta modes!
    let profile = NumericalProfile::reference_f32().expect("profile");
    for mode in ["sensor-neighbor-mean-v1", "direct-lateral-inhibition-v1"] {
        let toml = ecology_toml(
            "field-ecology-v2",
            small_overrides(),
            &format!(
                r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 2.0
sweeps = 2

[v2]
profile = "minimal"

[v2.differentiation]
enabled = true
state_width = 2
epigenetic_gate_bias = 1.0
state_inheritance = "asymmetric-stem-v1"
inheritance_retention = 0.85
inheritance_jitter = 0.05
stem_asymmetry = -0.8

[v2.differentiation.notch_delta]
enabled = true
mode = "{mode}"
inhibition_gain = 0.25
"#
            ),
        );
        let cfg = ValidatedConfig::from_toml(&toml).expect("nd cfg");
        let mut world = World::found(&cfg, Arrangement::Compact, profile).expect("found");
        let watched_id = world.population().iter().next().expect("founder").id;
        world.watch(Some(watched_id));

        let mut newborn_id = None;
        for _ in 0..64 {
            let report = step_world(&mut world);
            // Verify watched activation trace succeeds every tick on live watched founder.
            if let Some(snap) = world.organism_snapshot(watched_id).expect("snapshot")
                && snap.organism.held.is_valid()
            {
                assert!(
                    snap.activations.is_some(),
                    "{mode}: watched organism_snapshot must verify recurrent + notch-delta trace"
                );
            }
            if let Some(born) = report.births.first() {
                let child = world.population().get(born.child).expect("newborn alive");
                assert!(
                    !child.held.is_valid(),
                    "{mode}: C.13 newborn must be unevaluated"
                );
                assert_eq!(child.held.evaluated_at(), None);
                assert_eq!(child.held.recurrent_state().len(), 2);
                assert!(
                    child.held.recurrent_state().iter().any(|&v| v != 0.0),
                    "{mode}: asymmetric-stem-v1 newborn must inherit nonzero epigenetic state"
                );
                assert_eq!(child.last_neighbor_h0, 0.0);
                newborn_id = Some(born.child);
                break;
            }
        }
        let child_id = newborn_id.expect("expected a birth within 64 ticks");
        let expected_child_state = world
            .population()
            .get(child_id)
            .expect("child")
            .held
            .recurrent_state()
            .to_vec();
        let expected_cell_states = world.cell_states().expect("cell_states");
        let expected_neighbors: Vec<(OrganismId, f32)> = world
            .population()
            .iter()
            .map(|o| (o.id, o.last_neighbor_h0))
            .collect();

        // Checkpoint v3 write -> restore bit-for-bit (`INV-14`, `C.13`, `Trap F`).
        let mut writer = CheckpointWriter::new(&cfg.config().model);
        world.write_checkpoint(&mut writer).expect("write ckpt");
        let bytes = writer.finish().expect("finish");
        let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("parse");
        let mut restored = World::read_checkpoint(&reader, profile).expect("read ckpt");

        assert_eq!(
            world.digest(),
            restored.digest(),
            "{mode}: checkpoint v3 digest mismatch"
        );
        assert_eq!(
            restored.cell_states().expect("restored cell_states"),
            expected_cell_states
        );
        let restored_child = restored.population().get(child_id).expect("restored child");
        assert_eq!(restored_child.held.evaluated_at(), None);
        assert_eq!(
            restored_child.held.recurrent_state(),
            expected_child_state.as_slice()
        );
        for (id, hbar) in expected_neighbors {
            let got = restored
                .population()
                .get(id)
                .expect("restored org")
                .last_neighbor_h0;
            assert_eq!(
                got.to_bits(),
                hbar.to_bits(),
                "{mode}: Trap F last_neighbor_h0 drifted for {id}"
            );
        }

        // Post-restore replay stays bit-identical for 16 more ticks.
        for post in 0..16 {
            step_world(&mut world);
            step_world(&mut restored);
            assert_eq!(
                world.digest(),
                restored.digest(),
                "{mode}: post-restore replay diverged at step {post}"
            );
        }
    }
}

#[test]
fn disabled_epigenetic_and_notch_delta_zero_overhead_and_v1_exact() {
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

[v2.differentiation]
enabled = true
state_width = 2

[v2.differentiation.notch_delta]
enabled = true
"#,
    );
    let v1_cfg = ValidatedConfig::from_toml(&v1_toml).expect("v1");
    let v2_off_cfg = ValidatedConfig::from_toml_with_overrides(
        &v2_toml,
        &[
            FeatureOverride::Disable(V2Feature::EpigeneticRecurrence.slug().to_owned()),
            FeatureOverride::Disable(V2Feature::NotchDelta.slug().to_owned()),
        ],
    )
    .expect("v2 off");
    let mut w_v1 = World::found(&v1_cfg, Arrangement::Compact, profile).expect("v1");
    let mut w_off = World::found(&v2_off_cfg, Arrangement::Compact, profile).expect("v2 off");

    assert_eq!(w_off.cell_states(), None);
    assert!(!w_off.population().tracks_notch_delta());
    assert_eq!(
        w_off.footprint().state,
        w_v1.footprint().state,
        "disabled epigenetic-recurrence + notch-delta must allocate 0 B extra state"
    );
    assert_eq!(
        w_off.footprint().scratch,
        w_v1.footprint().scratch,
        "disabled epigenetic-recurrence + notch-delta must allocate 0 B extra scratch"
    );
    assert_eq!(w_off.digest(), w_v1.digest());

    for tick in 0..24 {
        step_world(&mut w_v1);
        step_world(&mut w_off);
        assert_eq!(
            w_off.digest(),
            w_v1.digest(),
            "disabled V2-12 diverged from v1-contact at tick {tick}"
        );
    }

    // Verify `intervention::IntroduceOrganisms` on an active V2-12 world initializes
    // `HeldOutput::INVALID` and `last_neighbor_h0 == 0.0`.
    let v2_on_cfg = ValidatedConfig::from_toml(&v2_toml).expect("v2 on");
    let mut w_on = World::found(&v2_on_cfg, Arrangement::Compact, profile).expect("v2 on");
    let Advance::Ready(cand) = w_on
        .advance_with(&[Intervention::IntroduceOrganisms {
            cell: [4, 4],
            count: 2,
            energy: 2.0,
        }])
        .expect("introduce")
    else {
        panic!("unexpected pause");
    };
    let report = cand.publish().expect("publish");
    assert_eq!(report.introductions.len(), 2);
    for intro in &report.introductions {
        let org = w_on.population().get(intro.organism).expect("introduced");
        assert_eq!(org.held, HeldOutput::INVALID);
        assert_eq!(org.last_neighbor_h0, 0.0);
    }
}

#[test]
fn e2e_clonal_core_sheath_differentiation_with_waste() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 1.6
sweeps = 2

[v2]
profile = "minimal"

[v2.motor]
enabled = true

[v2.crowding]
enabled = true
mode = "local-neighbors-v1"
expose_sensor = true

[v2.metabolism.lysis]
enabled = true

[v2.metabolism.waste]
enabled = true
byproduct_recycle_fraction = 0.45

[v2.differentiation]
enabled = true
state_width = 2
epigenetic_gate_bias = 2.5
state_inheritance = "asymmetric-stem-v1"
inheritance_retention = 0.8
inheritance_jitter = 0.05
stem_asymmetry = -0.8
affinity_rotation_enabled = true
emt_adhesion_enabled = true

[v2.differentiation.notch_delta]
enabled = true
mode = "sensor-neighbor-mean-v1"
"#,
    );
    let cfg = ValidatedConfig::from_toml(&toml).expect("bundle-a-like cfg");
    let abi = cfg.v2_abi();
    assert_eq!(
        abi.input_width, 108,
        "104 + c_den(1) + h_bar(1) + S(2) = 108"
    );
    assert_eq!(abi.action_width, 40, "36 + so2-motor(4) = 40");
    assert_eq!(abi.raw_output_width, 44, "40 + 2*S(4) = 44");
    assert_eq!(abi.input_width % 4, 0);
    assert_eq!(abi.raw_output_width % 4, 0);

    let ablated_cfg = ValidatedConfig::from_toml_with_overrides(
        &toml,
        &[
            FeatureOverride::Disable(V2Feature::EpigeneticRecurrence.slug().to_owned()),
            FeatureOverride::Disable(V2Feature::NotchDelta.slug().to_owned()),
        ],
    )
    .expect("ablated cfg");

    let mut compact = World::found(&cfg, Arrangement::Compact, profile).expect("compact");
    let mut freelist = World::found(&cfg, Arrangement::FreeList, profile).expect("freelist");
    let mut parallel = World::found(&cfg, Arrangement::Compact, profile).expect("parallel");
    let mut batched = World::found(&cfg, Arrangement::Compact, profile).expect("batched");
    let mut ablated = World::found(&ablated_cfg, Arrangement::Compact, profile).expect("ablated");

    parallel
        .set_cpu_workers(NonZeroUsize::new(4).expect("4"))
        .expect("4 workers");
    batched.set_controller_backend(Some(Box::new(DynamicReferenceBatchBackend {
        expected_input_width: 108,
        expected_raw_output_width: 44,
        scratch: ControllerScratch::default(),
    })));

    let admission = compact.admit_memory(false, 0, 0).expect("admit_memory");
    assert!(admission.peak_bytes <= admission.budget_bytes);
    let footprint_before = compact.footprint();

    let mut total_births = 0_usize;
    let mut max_rel_residual = 0.0_f64;
    for tick in 0..96 {
        let rep = step_world(&mut compact);
        step_world(&mut freelist);
        step_world(&mut parallel);
        step_world(&mut batched);
        step_world(&mut ablated);

        total_births += rep.births.len();
        let rel = rep.closure.residual().abs() / rep.closure.scale().max(1.0);
        max_rel_residual = max_rel_residual.max(rel);
        assert!(
            rel <= 1e-6,
            "INV-03 per-tick relative residual {rel:e} exceeded 1e-6 (§21.4) at tick {tick}"
        );
        assert_eq!(compact.digest(), freelist.digest(), "C.7 at tick {tick}");
        assert_eq!(compact.digest(), parallel.digest(), "C.8 at tick {tick}");
        assert_eq!(
            compact.digest(),
            batched.digest(),
            "batched recurrent backend diverged from reference at tick {tick}"
        );
    }

    let cum_norm = compact.ledger().report().normalized.unwrap_or(0.0);
    assert!(
        cum_norm <= profile.residual.cumulative(),
        "INV-03 cumulative normalized residual {cum_norm:e} exceeded {}",
        profile.residual.cumulative()
    );
    assert!(max_rel_residual <= 1e-6);

    let ctrl_report = batched.controller_report();
    assert!(ctrl_report.batched > 0, "batched route must evaluate jobs");
    assert_eq!(
        ctrl_report.deferred, 0,
        "108/44 recurrent ABI must incur 0 deferrals"
    );
    assert_eq!(ctrl_report.failure, None);

    assert!(total_births > 0, "expected births over 96 ticks");
    assert_ne!(
        compact.digest(),
        ablated.digest(),
        "epigenetic differentiation + notch-delta must diverge from disabled ablation"
    );
    let states = compact.cell_states().expect("cell_states");
    assert_eq!(states.len(), compact.population().len());
    assert!(
        states
            .iter()
            .all(|&h| h.is_finite() && (-1.0_f32..=1.0_f32).contains(&h))
    );
    let min_h = states.iter().copied().fold(f32::INFINITY, f32::min);
    let max_h = states.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    assert!(
        max_h - min_h > 1e-3,
        "live population must exhibit differentiated cell states h_0: min={min_h} max={max_h}"
    );

    // Bounded footprint never grew during live stepping.
    let footprint_after = compact.footprint();
    assert_eq!(footprint_after.field, footprint_before.field);
    assert_eq!(footprint_after.scratch, footprint_before.scratch);
    assert_eq!(footprint_after.staging, footprint_before.staging);
    assert!(footprint_after.state <= footprint_before.state);
}
