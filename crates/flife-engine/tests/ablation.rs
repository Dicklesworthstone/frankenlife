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

use flife_core::config::ChemistryConfig;
use flife_core::model::{
    FIELD_ECOLOGY_V1, FIELD_ECOLOGY_V1_CHEMISTRY_ONLY, FIELD_ECOLOGY_V1_FRESH_OFFSPRING,
    FIELD_ECOLOGY_V1_UNGATED_MOTION,
};
use flife_core::rng::SemanticRng;
use flife_core::{Flow, ValidatedConfig};
use flife_engine::{Arrangement, ModelVariant, NumericalProfile, Stepped, TickReport, World};
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
    ] {
        assert_eq!(ModelVariant::of(name), Ok(variant));
        // Each validates under its registered name.
        let config = ecology(name, &[]);
        assert_eq!(config.config().model, name);
    }
    assert!(ModelVariant::of("field-ecology-v2").is_err());
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
