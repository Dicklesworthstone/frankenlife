//! FL-041 evidence (plan §15.3, §21.6, H2): the ablation models change
//! exactly the rule they name.
//!
//! - `field-ecology-v1-chemistry-only` evaluates and charges every
//!   controller as `field-ecology-v1` does, and decodes its actions from an
//!   all-zero raw output. The oracle is the plan's decoding (§6.3) of zero:
//!   `σ(0) = 1/2` everywhere, so the emission gate sits exactly at Appendix
//!   A's threshold of 1/2 and never opens, and every birth gives its child
//!   the midpoint share `β = 0.1 + 0.8 / 2 = 0.5` of what remains after the
//!   birth cost.
//! - `field-ecology-v1-fresh-offspring` gives each newborn the genome a
//!   founder with the newborn's identity would have (A.3 items 2–3). The
//!   oracle builds that founder genome directly with `flife_genome::founder`
//!   and compares it bit for bit; nothing of the parent's parameters
//!   survives in it.
//!
//! - `field-ecology-v1-ungated-motion` (plan §5.4) follows the field with
//!   gain 1 instead of the tick's uptake ratio. The oracle is §5.4's first
//!   step from rest, computed here with the platform's `exp`: with no uptake
//!   capacity the baseline's organisms stay put and the variant's move by
//!   exactly that step.
//!
//! Each is compared with `field-ecology-v1` from the same founding, so a
//! test that would also pass on the baseline proves nothing: each has a
//! baseline counterpart that must differ.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::collections::BTreeMap;

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::config::ChemistryConfig;
use flife_core::model::{
    FIELD_ECOLOGY_V1, FIELD_ECOLOGY_V1_CHEMISTRY_ONLY, FIELD_ECOLOGY_V1_CONTACT,
    FIELD_ECOLOGY_V1_FRESH_OFFSPRING, FIELD_ECOLOGY_V1_UNGATED_MOTION, FIELD_ECOLOGY_V2,
    FIELD_ECOLOGY_V2_CHEMISTRY_ONLY, FIELD_ECOLOGY_V2_FIXED_CONTROLLER,
    FIELD_ECOLOGY_V2_FRESH_OFFSPRING,
};
use flife_core::rng::SemanticRng;
use flife_core::{FeatureOverride, Flow, ValidatedConfig};
use flife_engine::{
    Arrangement, ModelVariant, NumericalProfile, Stepped, TickReport, WORLD_CHECKPOINT_CHUNKS,
    World,
};
use flife_genome::{GenomeQuota, TopologyCatalog, TraitLimits, founder, initial_scale};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// Appendix A's 32 × 32 test ecology (the FL-013 world tests' values) under
/// `model`, with `extra` replacing `(section, key, value)`.
fn ecology(model: &str, extra: &[(&str, &str, &str)]) -> ValidatedConfig {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    let mut overrides = vec![
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "1024"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "120.0"),
        ("sources", "sigma_world_units", "3.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.6"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.2",
        ),
        ("reproduction", "maximum_attempt_rate_per_second", "4.0"),
        ("reproduction", "cooldown_seconds", "0.25"),
    ];
    overrides.extend_from_slice(extra);
    for (section, key, value) in overrides {
        let header = format!("[{section}]");
        let first = lines
            .iter()
            .position(|line| line.trim() == header)
            .expect("a section");
        let line = lines[first + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| first + 1 + offset)
            .expect("a key");
        lines[line] = format!("{key} = {value}");
    }
    let model_line = lines
        .iter()
        .position(|line| line.starts_with("model = "))
        .expect("the model key");
    lines[model_line] = format!("model = \"{model}\"");
    ValidatedConfig::from_toml(&lines.join("\n")).expect("a valid configuration")
}

fn found(config: &ValidatedConfig) -> World {
    World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("the world founds")
}

fn publish(world: &mut World) -> TickReport {
    match world.step().expect("the tick advances") {
        Stepped::Published(report) => *report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

/// Every parameter bit pattern of every live organism, in identity order.
fn genomes(world: &World) -> Vec<(u64, Vec<u32>)> {
    world
        .population()
        .iter()
        .map(|organism| {
            let genome = world.population().genome(organism.id).expect("a genome");
            (
                organism.id.get(),
                genome
                    .parameters()
                    .iter()
                    .map(|value| value.to_bits())
                    .collect(),
            )
        })
        .collect()
}

/// What 240 ticks of a world did: emitters, and each birth's child share
/// `child / (child + parent)` of the committed energies right after it.
struct Course {
    emitters: usize,
    evaluations: usize,
    shares: Vec<f64>,
}

fn course(world: &mut World, ticks: usize) -> Course {
    let mut emitters = 0;
    let mut evaluations = 0;
    let mut shares = Vec::new();
    for _ in 0..ticks {
        let report = publish(world);
        emitters += report.emitted;
        evaluations += report.evaluated;
        for birth in &report.births {
            let child = world.population().get(birth.child).expect("the child");
            let parent = world.population().get(birth.parent).expect("the parent");
            shares.push(child.energy / (child.energy + parent.energy));
        }
    }
    Course {
        emitters,
        evaluations,
        shares,
    }
}

#[test]
fn the_model_key_names_each_variant_and_refuses_others() {
    for (name, variant) in [
        (FIELD_ECOLOGY_V1, ModelVariant::Baseline),
        (FIELD_ECOLOGY_V1_CHEMISTRY_ONLY, ModelVariant::ChemistryOnly),
        (
            FIELD_ECOLOGY_V1_FRESH_OFFSPRING,
            ModelVariant::FreshOffspring,
        ),
        (FIELD_ECOLOGY_V1_UNGATED_MOTION, ModelVariant::UngatedMotion),
        (FIELD_ECOLOGY_V2, ModelVariant::V2),
        (
            FIELD_ECOLOGY_V2_CHEMISTRY_ONLY,
            ModelVariant::V2ChemistryOnly,
        ),
        (
            FIELD_ECOLOGY_V2_FRESH_OFFSPRING,
            ModelVariant::V2FreshOffspring,
        ),
        (
            FIELD_ECOLOGY_V2_FIXED_CONTROLLER,
            ModelVariant::V2FixedController,
        ),
    ] {
        assert_eq!(ModelVariant::of(name), Ok(variant));
        // Each validates under its registered name.
        let config = ecology(name, &[]);
        assert_eq!(config.config().model, name);
    }
    assert!(ModelVariant::of("field-ecology-v99").is_err());
    let text = PLAN.replace(
        "model = \"field-ecology-v1\"",
        "model = \"field-ecology-v1-shuffled\"",
    );
    let appendix = text.find("## Appendix A").map_or("", |at| &text[at..]);
    let start = appendix.find("```toml\n").map_or(0, |at| at + 8);
    let length = appendix[start..].find("```").unwrap_or(0);
    assert!(
        ValidatedConfig::from_toml(&appendix[start..start + length]).is_err(),
        "an unregistered model is refused"
    );
}

#[test]
fn a_chemistry_only_world_pays_the_same_evaluations_and_decides_from_zero() {
    let mut baseline = found(&ecology(FIELD_ECOLOGY_V1, &[]));
    let mut ablated = found(&ecology(FIELD_ECOLOGY_V1_CHEMISTRY_ONLY, &[]));
    // One founding: the model name changes no founder.
    assert_eq!(genomes(&baseline), genomes(&ablated));

    // The first tick starts from identical state, and decisions act only
    // after the costs: every controller is evaluated and charged, every
    // sensor and every motion paid, exactly as in the baseline.
    let first = publish(&mut baseline);
    let other = publish(&mut ablated);
    assert_eq!(first.evaluated, other.evaluated);
    assert!(first.evaluated > 0);
    for flow in [
        Flow::ControllerCost,
        Flow::SensorCost,
        Flow::MotionCost,
        Flow::Maintenance,
    ] {
        assert_eq!(
            first.closure.flow(flow).to_bits(),
            other.closure.flow(flow).to_bits(),
            "{flow:?}"
        );
    }
    assert!(first.closure.flow(Flow::ControllerCost) > 0.0);

    let baseline = course(&mut baseline, 239);
    let ablated = course(&mut ablated, 239);
    // σ(0) = 1/2 is not above Appendix A's gate threshold of 1/2.
    assert_eq!(ablated.emitters + other.emitted, 0);
    assert!(baseline.emitters + first.emitted > 0, "the baseline emits");
    assert!(ablated.evaluations > 0);
    // Every birth splits at β = 0.1 + 0.8 σ(0) = 1/2 (rounded split, so
    // within a relative 1e-12); the baseline's controllers choose others.
    assert!(ablated.shares.len() >= 5, "{} births", ablated.shares.len());
    for share in &ablated.shares {
        assert!((share - 0.5).abs() < 1.0e-12, "share {share}");
    }
    assert!(baseline.shares.len() >= 5);
    assert!(
        baseline
            .shares
            .iter()
            .any(|share| (share - 0.5).abs() > 1.0e-3),
        "the baseline's shares vary"
    );
}

#[test]
fn a_fresh_offspring_child_holds_the_founder_genome_of_its_own_identity() {
    for (model, fresh) in [
        (FIELD_ECOLOGY_V1_FRESH_OFFSPRING, true),
        (FIELD_ECOLOGY_V1, false),
    ] {
        let config = ecology(model, &[]);
        let mut world = found(&config);
        let rng = SemanticRng::from_config(&config).expect("rng");
        let catalog = TopologyCatalog::from_config(&config).expect("catalog");
        let limits = TraitLimits::from_config(&config).expect("limits");
        let mut quota = GenomeQuota::from_config(&config).expect("quota");
        let mut checked = 0;
        let mut identical_to_parent = 0_u64;
        let mut compared = 0_u64;
        for _ in 0..240 {
            let report = publish(&mut world);
            for birth in &report.births {
                let child = world
                    .population()
                    .genome(birth.child)
                    .expect("the child's genome");
                let oracle = founder(
                    &rng,
                    birth.child,
                    birth.genome,
                    &catalog,
                    &mut quota,
                    &limits,
                    initial_scale(&config),
                )
                .expect("the founder genome");
                let same = child.topology() == oracle.topology()
                    && child
                        .parameters()
                        .iter()
                        .map(|v| v.to_bits())
                        .eq(oracle.parameters().iter().map(|v| v.to_bits()))
                    && child.traits() == oracle.traits();
                assert_eq!(
                    same, fresh,
                    "{model}: child {} against its founder genome",
                    birth.child
                );
                if fresh {
                    assert_eq!(
                        (
                            birth.record.perturbed,
                            birth.record.replaced,
                            birth.record.topology
                        ),
                        (0, 0, None)
                    );
                }
                // A genome never changes, so the parent's, read after the
                // tick, is the one the child was made from.
                let parent = world.population().genome(birth.parent).expect("the parent");
                if parent.topology() == child.topology() {
                    compared += u64::try_from(parent.parameters().len()).expect("a count");
                    identical_to_parent += u64::try_from(
                        parent
                            .parameters()
                            .bits()
                            .zip(child.parameters().bits())
                            .filter(|(a, b)| a == b)
                            .count(),
                    )
                    .expect("a count");
                }
                checked += 1;
            }
        }
        assert!(checked >= 5, "{model}: {checked} births");
        // Inherited children copy all but a few parameters; fresh children
        // share little more than the founders' zero biases with their
        // parents.
        if fresh {
            assert!(
                identical_to_parent * 4 < compared,
                "{identical_to_parent} of {compared} identical"
            );
        } else {
            assert!(
                identical_to_parent * 10 > compared * 9,
                "{identical_to_parent} of {compared} identical"
            );
        }
    }
}

/// §5.4's step from rest, from the plan with the platform's `exp`: the
/// response `gain Σ_k χ_k ∇_k` clamped radially to the maximum
/// acceleration, times the drag integral `(1 − e^(−γΔt)) / γ`, clamped
/// radially to the maximum speed.
fn step_from_rest(
    gain: f64,
    response: &[f64],
    gradients: &[[f64; 2]],
    seconds: f64,
    chemistry: &ChemistryConfig,
) -> [f64; 2] {
    let clamp = |v: [f64; 2], limit: f64| {
        let norm = v[0].hypot(v[1]);
        if norm > limit {
            [v[0] * limit / norm, v[1] * limit / norm]
        } else {
            v
        }
    };
    assert_eq!(response.len(), gradients.len());
    let mut sum = [0.0_f64; 2];
    for (chi, gradient) in response.iter().zip(gradients) {
        sum[0] += chi * gradient[0];
        sum[1] += chi * gradient[1];
    }
    let force = clamp(
        [gain * sum[0], gain * sum[1]],
        chemistry.maximum_acceleration,
    );
    let gamma = chemistry.drag_per_second;
    let coefficient = if gamma == 0.0 {
        seconds
    } else {
        -(-gamma * seconds).exp_m1() / gamma
    };
    clamp(
        [coefficient * force[0], coefficient * force[1]],
        chemistry.maximum_speed,
    )
}

#[test]
fn an_ungated_motion_world_moves_an_organism_that_absorbed_nothing() {
    // No uptake capacity: no organism absorbs anything, so the baseline's
    // g_u is zero for all of them.
    let starved = [("chemistry", "uptake_capacity_per_second", "0.0")];
    let mut moved = 0;
    for (model, gated) in [
        (FIELD_ECOLOGY_V1, true),
        (FIELD_ECOLOGY_V1_UNGATED_MOTION, false),
    ] {
        let config = ecology(model, &starved);
        let chemistry = &config.config().chemistry;
        let world_config = &config.config().world;
        let seconds =
            f64::from(world_config.tick_numerator) / f64::from(world_config.tick_denominator);
        let ids: Vec<_> = found(&config).population().ids().take(12).collect();
        for id in ids {
            let mut world = found(&config);
            let from = world.population().get(id).expect("a founder");
            assert_eq!(from.velocity, [0.0, 0.0], "founders start at rest");
            world.watch(Some(id));
            publish(&mut world);
            let snapshot = world
                .organism_snapshot(id)
                .expect("inspects")
                .expect("lives");
            let sensed = snapshot.sensed.as_ref().expect("recorded");
            let step = snapshot.transactions.last().expect("a watched step");
            assert_eq!(sensed.tick, step.tick);
            assert_eq!(step.uptake.absorbed.to_bits(), 0.0_f64.to_bits());
            let motion = step.motion.expect("it was alive to move");
            let oracle = step_from_rest(
                if gated { 0.0 } else { 1.0 },
                snapshot.traits.motion_response(),
                &sensed.observation.gradients,
                seconds,
                chemistry,
            );
            if gated {
                assert_eq!(motion.proposed, [0.0, 0.0], "{model}: {id} stays put");
                assert_eq!(snapshot.organism.position, from.position);
                assert_eq!(motion.cost.requested.to_bits(), 0.0_f64.to_bits());
            } else {
                for axis in 0..2 {
                    let tolerance = 1e-12 * oracle[axis].abs().max(1e-300);
                    assert!(
                        (motion.proposed[axis] - oracle[axis]).abs() <= tolerance,
                        "{model}: {id} proposed {:?}, the oracle {oracle:?}",
                        motion.proposed
                    );
                }
                if oracle != [0.0, 0.0] {
                    moved += 1;
                    assert!(motion.cost.paid > 0.0, "{id} paid for its motion");
                    assert_ne!(snapshot.organism.position, from.position);
                }
            }
        }
    }
    assert!(moved >= 10, "only {moved} of 12 starved organisms moved");
}

/// A settle backend whose every stage fails, counting the calls that reach
/// it.
#[derive(Debug, Default)]
struct Refusing {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Refusing {
    fn refuse(&self) -> Result<(), flife_core::CoreError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(flife_core::CoreError::MissingCapability {
            capability: "a test backend's settle".into(),
        })
    }
}

impl flife_field::settle::SettleBackend for Refusing {
    fn describe(&self) -> String {
        "refusing".into()
    }
    fn chemistry(
        &mut self,
        _: &flife_field::FieldState,
        _: &flife_field::settle::ChemistryConstants,
        _: &[flife_field::settle::ChemistryOrganism],
        _: &[flife_field::settle::ChemistryPacket],
        _: &mut Vec<flife_field::settle::ChemistryOutcome>,
    ) -> Result<(), flife_core::CoreError> {
        self.refuse()
    }
    fn motion(
        &mut self,
        _: &flife_field::settle::MotionConstants,
        _: &[flife_field::settle::MotionOrganism],
        _: &mut Vec<flife_field::settle::MotionOutcome>,
    ) -> Result<(), flife_core::CoreError> {
        self.refuse()
    }
    fn emission(
        &mut self,
        _: &flife_field::FieldGrid,
        _: f64,
        _: &[flife_field::settle::EmissionOrganism],
        _: &mut Vec<flife_field::settle::EmissionOutcome>,
    ) -> Result<(), flife_core::CoreError> {
        self.refuse()
    }
    fn proposal(
        &mut self,
        _: &flife_field::settle::ProposalConstants,
        _: &[flife_field::settle::ProposalOrganism],
        _: &mut Vec<flife_field::settle::ProposalOutcome>,
    ) -> Result<(), flife_core::CoreError> {
        self.refuse()
    }
    fn mutation(
        &mut self,
        _: &flife_field::settle::MutationConstants,
        _: &[flife_field::settle::MutationBirth],
        _: flife_field::settle::MutationValues<'_>,
        _: &mut Vec<flife_field::settle::MutationCounts>,
    ) -> Result<(), flife_core::CoreError> {
        self.refuse()
    }
}

#[test]
fn a_settle_backend_never_settles_an_ungated_motion_tick() {
    // The port's motion scales by the uptake ratio and the host does not
    // recompute its velocities, so the variant must never reach it: the
    // first tick is refused before any dispatch, runs on the reference, and
    // the backend is dropped. The baseline's backend is called.
    for (model, ungated) in [
        (FIELD_ECOLOGY_V1_UNGATED_MOTION, true),
        (FIELD_ECOLOGY_V1, false),
    ] {
        let config = ecology(model, &[]);
        let mut reference = found(&config);
        let mut served = found(&config);
        let backend = Refusing::default();
        let calls = std::sync::Arc::clone(&backend.calls);
        served.set_settle_backend(Some(Box::new(backend)));
        for _ in 0..24 {
            publish(&mut reference);
            publish(&mut served);
        }
        assert_eq!(served.digest(), reference.digest(), "{model}");
        let report = served.settle_report();
        assert_eq!((report.ticks, report.reference_ticks), (0, 1), "{model}");
        let failure = report.failure.as_deref().expect("the backend was dropped");
        let calls = calls.load(std::sync::atomic::Ordering::SeqCst);
        if ungated {
            assert_eq!(calls, 0, "no stage reached the backend");
            assert!(
                failure.contains(FIELD_ECOLOGY_V1_UNGATED_MOTION),
                "{failure}"
            );
        } else {
            assert_eq!(calls, 1, "the baseline's chemistry reached it");
        }
    }
}

/// Build TOML text for `ecology` with optional appended sections (e.g. `[contact]`).
fn ecology_toml(model: &str, extra: &[(&str, &str, &str)], append: &str) -> String {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    let mut overrides = vec![
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "1024"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "120.0"),
        ("sources", "sigma_world_units", "3.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.6"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.2",
        ),
        ("reproduction", "maximum_attempt_rate_per_second", "4.0"),
        ("reproduction", "cooldown_seconds", "0.25"),
    ];
    overrides.extend_from_slice(extra);
    for (section, key, value) in overrides {
        let header = format!("[{section}]");
        let first = lines
            .iter()
            .position(|line| line.trim() == header)
            .expect("a section");
        let line = lines[first + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| first + 1 + offset)
            .expect("a key");
        lines[line] = format!("{key} = {value}");
    }
    let model_line = lines
        .iter()
        .position(|line| line.starts_with("model = "))
        .expect("the model key");
    lines[model_line] = format!("model = \"{model}\"");
    let mut text = lines.join("\n");
    if !append.is_empty() {
        text.push('\n');
        text.push_str(append);
    }
    text
}

/// V2-01 E2E: `field-ecology-v2 --v1-compat` matches `field-ecology-v1` bit
/// for bit over 256 ticks on `64×64`, and `field-ecology-v2` with `[contact]`
/// plus `--v1-compat` matches `field-ecology-v1-contact` bit for bit over 256
/// ticks on `64×64`.
#[test]
fn e2e_v2_disable_all_matches_v1_and_v1_contact_bit_for_bit() {
    let grid64 = [
        ("world", "width", "64.0"),
        ("world", "height", "64.0"),
        ("world", "grid_width", "64"),
        ("world", "grid_height", "64"),
        ("population", "initial_count", "64"),
        ("sources", "power_per_source", "240.0"),
    ];

    // 1. Pure-field v1 vs v2 --v1-compat.
    let v1_cfg =
        ValidatedConfig::from_toml(&ecology_toml(FIELD_ECOLOGY_V1, &grid64, "")).expect("v1");
    let v2_cfg = ValidatedConfig::from_toml_with_overrides(
        &ecology_toml(FIELD_ECOLOGY_V2, &grid64, ""),
        &[FeatureOverride::V1Compat],
    )
    .expect("v2 compat");
    assert!(v2_cfg.inv08_pure_field());
    assert_eq!(v2_cfg.v2_feature_bitmask(), 0);
    let mut w_v1 = found(&v1_cfg);
    let mut w_v2 = found(&v2_cfg);
    assert_eq!(w_v1.digest(), w_v2.digest(), "tick 0 pure-field parity");
    let mut births = 0usize;
    let mut emitted = 0usize;
    for tick in 1..=256 {
        let r1 = publish(&mut w_v1);
        let r2 = publish(&mut w_v2);
        births += r1.births.len();
        emitted += r1.emitted;
        assert_eq!(
            r1.births.len(),
            r2.births.len(),
            "birth count parity at tick {tick}"
        );
        assert_eq!(
            w_v1.digest(),
            w_v2.digest(),
            "pure-field WorldDigest parity at tick {tick}"
        );
    }
    assert!(births > 0, "expected births over 256 ticks");
    assert!(emitted > 0, "expected emissions over 256 ticks");

    // 2. Contact v1-contact vs v2 + [contact] --v1-compat.
    let contact =
        "\n[contact]\ndiameter = 0.25\nstiffness = 0.5\nadhesion = 0.25\nadhesion_range = 2.0\n";
    let v1c_cfg =
        ValidatedConfig::from_toml(&ecology_toml(FIELD_ECOLOGY_V1_CONTACT, &grid64, contact))
            .expect("v1-contact");
    let v2c_cfg = ValidatedConfig::from_toml_with_overrides(
        &ecology_toml(FIELD_ECOLOGY_V2, &grid64, contact),
        &[FeatureOverride::V1Compat],
    )
    .expect("v2-contact compat");
    assert!(!v2c_cfg.inv08_pure_field());
    assert_eq!(v2c_cfg.v2_feature_bitmask(), 0);
    let mut w_v1c = found(&v1c_cfg);
    let mut w_v2c = found(&v2c_cfg);
    assert_eq!(w_v1c.digest(), w_v2c.digest(), "tick 0 contact parity");
    for tick in 1..=256 {
        let _ = publish(&mut w_v1c);
        let _ = publish(&mut w_v2c);
        assert_eq!(
            w_v1c.digest(),
            w_v2c.digest(),
            "contact WorldDigest parity at tick {tick}"
        );
    }
}

/// V2-01 evidence for `field-ecology-v2-chemistry-only` and
/// `field-ecology-v2-fresh-offspring` (cited in `flife-core::registry`).
#[test]
fn v2_chemistry_only_and_fresh_offspring_controls() {
    let base_cfg = ValidatedConfig::from_toml_with_overrides(
        &ecology_toml(FIELD_ECOLOGY_V2, &[], ""),
        &[FeatureOverride::V1Compat],
    )
    .expect("v2 base");
    let chem_cfg = ValidatedConfig::from_toml_with_overrides(
        &ecology_toml(FIELD_ECOLOGY_V2_CHEMISTRY_ONLY, &[], ""),
        &[FeatureOverride::V1Compat],
    )
    .expect("v2 chemistry-only");
    let mut baseline = found(&base_cfg);
    let mut ablated = found(&chem_cfg);
    assert_eq!(genomes(&baseline), genomes(&ablated));

    let first = publish(&mut baseline);
    let other = publish(&mut ablated);
    assert_eq!(first.evaluated, other.evaluated);
    assert!(first.evaluated > 0);
    for flow in [
        Flow::ControllerCost,
        Flow::SensorCost,
        Flow::MotionCost,
        Flow::Maintenance,
    ] {
        assert_eq!(
            first.closure.flow(flow).to_bits(),
            other.closure.flow(flow).to_bits(),
            "{flow:?}"
        );
    }
    let baseline_course = course(&mut baseline, 239);
    let ablated_course = course(&mut ablated, 239);
    assert_eq!(ablated_course.emitters + other.emitted, 0);
    assert!(baseline_course.emitters + first.emitted > 0);
    assert!(ablated_course.shares.len() >= 5);
    for share in &ablated_course.shares {
        assert!((share - 0.5).abs() < 1.0e-12, "share {share}");
    }

    // Fresh offspring draws each child's genome as a founder at its own identity.
    let fresh_cfg = ValidatedConfig::from_toml_with_overrides(
        &ecology_toml(FIELD_ECOLOGY_V2_FRESH_OFFSPRING, &[], ""),
        &[FeatureOverride::V1Compat],
    )
    .expect("v2 fresh-offspring");
    let mut fresh_world = found(&fresh_cfg);
    let rng = SemanticRng::from_config(&fresh_cfg).expect("rng");
    let catalog = TopologyCatalog::from_config(&fresh_cfg).expect("catalog");
    let limits = TraitLimits::from_config(&fresh_cfg).expect("limits");
    let mut quota = GenomeQuota::from_config(&fresh_cfg).expect("quota");
    let scale = initial_scale(&fresh_cfg);
    let mut checked_births = 0usize;
    for _ in 0..240 {
        let report = publish(&mut fresh_world);
        for birth in &report.births {
            let child_genome = fresh_world
                .population()
                .genome(birth.child)
                .expect("child genome");
            let expected = founder(
                &rng,
                birth.child,
                birth.genome,
                &catalog,
                &mut quota,
                &limits,
                scale,
            )
            .expect("oracle founder");
            assert_eq!(child_genome.topology(), expected.topology());
            assert_eq!(child_genome.traits(), expected.traits());
            assert!(child_genome.parameters().same_bits(&expected.parameters()));
            checked_births += 1;
        }
        if checked_births >= 8 {
            break;
        }
    }
    assert!(
        checked_births >= 8,
        "expected at least 8 fresh-offspring births"
    );
}

/// V2-01 E2E: `field-ecology-v2-fixed-controller` preserves controller weights
/// and topology 100% across inheritance over 512 ticks while still mutating
/// chemical/sensing traits, and survives checkpoint v3 round-trip bit-for-bit.
#[test]
fn e2e_v2_fixed_controller_control_mutates_traits_not_weights() {
    let config = ValidatedConfig::from_toml_with_overrides(
        &ecology_toml(FIELD_ECOLOGY_V2_FIXED_CONTROLLER, &[], ""),
        &[FeatureOverride::V1Compat],
    )
    .expect("v2 fixed-controller");
    let profile = NumericalProfile::reference_f32().expect("profile");
    let mut world = World::found(&config, Arrangement::FreeList, profile).expect("found");

    // Snapshot every founder's controller topology + exact weight bits by lineage.
    let mut founder_controllers: BTreeMap<u64, (usize, Vec<u32>)> = BTreeMap::new();
    let mut known_traits: BTreeMap<u64, (Vec<u64>, Vec<u64>, u64)> = BTreeMap::new();
    for organism in world.population().iter() {
        let genome = world
            .population()
            .genome(organism.id)
            .expect("founder genome");
        let bits: Vec<u32> = genome.parameters().iter().map(f32::to_bits).collect();
        founder_controllers.insert(organism.lineage.get(), (genome.topology().index(), bits));
        let traits = genome.traits();
        known_traits.insert(
            organism.id.get(),
            (
                traits.affinity().iter().map(|v| v.to_bits()).collect(),
                traits
                    .motion_response()
                    .iter()
                    .map(|v| v.to_bits())
                    .collect(),
                traits.sensing_scale().to_bits(),
            ),
        );
    }

    let mut total_births = 0usize;
    let mut trait_mutations = 0usize;
    let mut checkpoint_bytes = None;

    for tick in 1..=512 {
        let report = publish(&mut world);
        for birth in &report.births {
            total_births += 1;
            let child = world.population().get(birth.child).expect("child row");
            let genome = world
                .population()
                .genome(birth.child)
                .expect("child genome");
            let child_bits: Vec<u32> = genome.parameters().iter().map(f32::to_bits).collect();
            let (want_topo, want_bits) = founder_controllers
                .get(&child.lineage.get())
                .expect("lineage recorded");
            assert_eq!(
                genome.topology().index(),
                *want_topo,
                "fixed-controller must preserve topology for child {}",
                birth.child
            );
            assert_eq!(
                &child_bits, want_bits,
                "fixed-controller must preserve 100% of weights for child {}",
                birth.child
            );
            let traits = genome.traits();
            let child_traits = (
                traits
                    .affinity()
                    .iter()
                    .map(|v| v.to_bits())
                    .collect::<Vec<_>>(),
                traits
                    .motion_response()
                    .iter()
                    .map(|v| v.to_bits())
                    .collect::<Vec<_>>(),
                traits.sensing_scale().to_bits(),
            );
            if let Some(parent_traits) = known_traits.get(&birth.parent.get())
                && *parent_traits != child_traits
            {
                trait_mutations += 1;
            }
            known_traits.insert(birth.child.get(), child_traits);
        }
        if tick == 256 {
            let mut writer = CheckpointWriter::new(&config.config().model);
            world
                .write_checkpoint(&mut writer)
                .expect("writes v3 checkpoint");
            checkpoint_bytes = Some(writer.finish().expect("finishes checkpoint"));
        }
    }

    assert!(
        total_births >= 10,
        "expected >= 10 births, got {total_births}"
    );
    assert!(
        trait_mutations > 0,
        "expected > 0 trait mutations under fixed-controller"
    );

    // Restore tick-256 v3 checkpoint and step to tick 512: must match bit-for-bit.
    let bytes = checkpoint_bytes.expect("checkpoint at tick 256");
    let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("parses v3");
    let mut resumed = World::read_checkpoint(&reader, profile).expect("restores v3");
    assert_eq!(resumed.tick().get(), 256);
    for _ in 257..=512 {
        publish(&mut resumed);
    }
    assert_eq!(
        resumed.digest(),
        world.digest(),
        "v3 checkpoint resume must reproduce tick-512 WorldDigest bit-for-bit"
    );
}
