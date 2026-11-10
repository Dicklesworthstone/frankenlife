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
//! Both are compared with `field-ecology-v1` from the same founding, so a
//! test that would also pass on the baseline proves nothing: each has a
//! baseline counterpart that must differ.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use flife_core::model::{
    FIELD_ECOLOGY_V1, FIELD_ECOLOGY_V1_CHEMISTRY_ONLY, FIELD_ECOLOGY_V1_FRESH_OFFSPRING,
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
                            .iter()
                            .zip(child.parameters())
                            .filter(|(a, b)| a.to_bits() == b.to_bits())
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
