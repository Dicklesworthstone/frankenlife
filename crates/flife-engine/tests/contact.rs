//! Evidence for the contact models (plan §5.4's "direct collisions … define
//! later model variants"): `field-ecology-v1-contact` adds exactly one rule
//! to `field-ecology-v1`, and its two controls add it to theirs.
//!
//! The oracle is the rule written out over every pair (`flife_engine::contact`
//! sums the same terms over neighbour bins): each survivor of a tick moves by
//! `Σ_j (k/2)(D − d_ij) u_ij` over the survivors `j` closer than the body
//! diameter `D`, clamped to `min(D/2, h/2)`. Each contact world is compared
//! with its model without contact from the same founding. On the first tick
//! the two must spend, emit and give birth identically, keep every velocity,
//! and differ only by that displacement of the survivors.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::collections::{BTreeMap, BTreeSet};

use flife_core::ValidatedConfig;
use flife_core::model::{
    FIELD_ECOLOGY_V1, FIELD_ECOLOGY_V1_CHEMISTRY_ONLY, FIELD_ECOLOGY_V1_CONTACT,
    FIELD_ECOLOGY_V1_CONTACT_CHEMISTRY_ONLY, FIELD_ECOLOGY_V1_CONTACT_FRESH_OFFSPRING,
    FIELD_ECOLOGY_V1_FRESH_OFFSPRING,
};
use flife_engine::contact::CONTACT_STIFFNESS;
use flife_engine::{Arrangement, ModelVariant, NumericalProfile, Stepped, TickReport, World};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// The world side and grid, in world units and cells.
const SIDE: f64 = 16.0;
/// Field cells a side.
const CELLS: f64 = 16.0;
/// The body diameter, `reproduction.child_position_offset_radius`.
const DIAMETER: f64 = 0.6;

/// Appendix A's starter at 16 × 16 with 640 founders under `model`: crowded
/// enough that uniform founders overlap.
fn ecology(model: &str) -> ValidatedConfig {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    let overrides = [
        ("world", "width", "16.0"),
        ("world", "height", "16.0"),
        ("world", "grid_width", "16"),
        ("world", "grid_height", "16"),
        ("population", "initial_count", "640"),
        ("population", "maximum_count", "4096"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "2"),
        ("sources", "power_per_source", "120.0"),
        ("sources", "sigma_world_units", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.6"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.2",
        ),
        ("reproduction", "maximum_attempt_rate_per_second", "4.0"),
        ("reproduction", "cooldown_seconds", "0.25"),
        ("reproduction", "child_position_offset_radius", "0.6"),
    ];
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

/// Each live organism's position, velocity and energy bits, by identity.
fn state(world: &World) -> BTreeMap<u64, ([f64; 2], [u64; 2], u64)> {
    world
        .population()
        .iter()
        .map(|organism| {
            (
                organism.id.get(),
                (
                    organism.position,
                    [
                        organism.velocity[0].to_bits(),
                        organism.velocity[1].to_bits(),
                    ],
                    organism.energy.to_bits(),
                ),
            )
        })
        .collect()
}

fn image(difference: f64) -> f64 {
    if difference >= 0.5 * SIDE {
        difference - SIDE
    } else if difference < -0.5 * SIDE {
        difference + SIDE
    } else {
        difference
    }
}

/// The rule over every pair of `survivors` (identity → settled position).
fn oracle(survivors: &BTreeMap<u64, [f64; 2]>) -> BTreeMap<u64, [f64; 2]> {
    let limit = (0.5 * DIAMETER).min(0.5 * SIDE / CELLS);
    survivors
        .iter()
        .map(|(&id, &[x, y])| {
            let mut sum = [0.0, 0.0];
            for (&other, &[ox, oy]) in survivors {
                if other == id {
                    continue;
                }
                let (dx, dy) = (image(x - ox), image(y - oy));
                let distance = dx.hypot(dy);
                if distance >= DIAMETER {
                    continue;
                }
                let push = 0.5 * CONTACT_STIFFNESS * (DIAMETER - distance);
                let (ux, uy) = if distance > 0.0 {
                    (dx / distance, dy / distance)
                } else if id > other {
                    (1.0, 0.0)
                } else {
                    (-1.0, 0.0)
                };
                sum[0] += push * ux;
                sum[1] += push * uy;
            }
            let length = sum[0].hypot(sum[1]);
            if length > limit {
                sum = [sum[0] * limit / length, sum[1] * limit / length];
            }
            (id, sum)
        })
        .collect()
}

/// The summed overlap `Σ_{i<j} (D − d_ij)₊` of the live population.
fn overlap(world: &World) -> f64 {
    let positions: Vec<[f64; 2]> = world.population().iter().map(|o| o.position).collect();
    let mut total = 0.0;
    for (index, &[x, y]) in positions.iter().enumerate() {
        for &[ox, oy] in &positions[index + 1..] {
            total += (DIAMETER - image(x - ox).hypot(image(y - oy))).max(0.0);
        }
    }
    total
}

/// One tick of `plain` and of `contact` from one founding: the same flows,
/// births, energies and velocities, and positions apart by exactly the
/// oracle's displacement of the survivors.
fn first_tick_differs_only_by_contact(plain: &str, contact: &str) {
    let mut without = found(&ecology(plain));
    let mut with = found(&ecology(contact));
    assert_eq!(state(&without), state(&with), "one founding");
    let report_without = publish(&mut without);
    let report_with = publish(&mut with);
    assert_eq!(
        format!("{:?}", report_without.closure),
        format!("{:?}", report_with.closure),
        "contact changes no energy flow of the tick"
    );
    let born: BTreeSet<u64> = report_without
        .births
        .iter()
        .map(|b| b.child.get())
        .collect();
    let born_with: BTreeSet<u64> = report_with.births.iter().map(|b| b.child.get()).collect();
    // Founders cool down through the first tick, so it has no births; were
    // there any, their children would not be displaced until the next.
    assert_eq!(born, born_with, "the same births");
    let before = state(&without);
    let after = state(&with);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    let survivors: BTreeMap<u64, [f64; 2]> = before
        .iter()
        .filter(|(id, _)| !born.contains(id))
        .map(|(&id, &(position, _, _))| (id, position))
        .collect();
    let displacement = oracle(&survivors);
    let mut moved = 0;
    for (id, &(position, velocity, energy)) in &before {
        let (contacted, contacted_velocity, contacted_energy) = after[id];
        assert_eq!(velocity, contacted_velocity, "{id}: contact keeps velocity");
        assert_eq!(energy, contacted_energy, "{id}: contact keeps energy");
        let [dx, dy] = displacement.get(id).copied().unwrap_or([0.0, 0.0]);
        if dx != 0.0 || dy != 0.0 {
            moved += 1;
        }
        let expected = [
            (position[0] + dx).rem_euclid(SIDE),
            (position[1] + dy).rem_euclid(SIDE),
        ];
        for axis in 0..2 {
            let error = image(contacted[axis] - expected[axis]).abs();
            assert!(
                error < 1e-12,
                "{id}: {contacted:?}, expected {expected:?} (moved by {dx}, {dy})"
            );
        }
    }
    assert!(moved > 100, "crowded founders should touch: {moved} moved");
}

#[test]
fn a_contact_world_separates_overlapping_organisms_and_keeps_the_ledger() {
    first_tick_differs_only_by_contact(FIELD_ECOLOGY_V1, FIELD_ECOLOGY_V1_CONTACT);

    // Over 64 ticks the contact world's bodies overlap less than half as much
    // (about a third here), and its ledger closes every tick.
    let mut without = found(&ecology(FIELD_ECOLOGY_V1));
    let mut with = found(&ecology(FIELD_ECOLOGY_V1_CONTACT));
    for _ in 0..64 {
        publish(&mut without);
        publish(&mut with);
    }
    let (plain, contacted) = (overlap(&without), overlap(&with));
    assert!(
        contacted < 0.5 * plain,
        "overlap {contacted} with contact, {plain} without"
    );
}

#[test]
fn the_contact_controls_combine_contact_with_their_one_rule() {
    for (name, variant) in [
        (FIELD_ECOLOGY_V1_CONTACT, ModelVariant::Contact),
        (
            FIELD_ECOLOGY_V1_CONTACT_CHEMISTRY_ONLY,
            ModelVariant::ContactChemistryOnly,
        ),
        (
            FIELD_ECOLOGY_V1_CONTACT_FRESH_OFFSPRING,
            ModelVariant::ContactFreshOffspring,
        ),
    ] {
        assert_eq!(ModelVariant::of(name), Ok(variant));
        assert!(variant.contact());
    }
    assert!(ModelVariant::ContactChemistryOnly.decides_from_zero());
    assert!(!ModelVariant::ContactChemistryOnly.fresh_offspring());
    assert!(ModelVariant::ContactFreshOffspring.fresh_offspring());
    assert!(!ModelVariant::ContactFreshOffspring.decides_from_zero());
    assert!(!ModelVariant::Contact.decides_from_zero());
    assert!(!ModelVariant::Contact.fresh_offspring());
    for plain in [
        ModelVariant::Baseline,
        ModelVariant::ChemistryOnly,
        ModelVariant::FreshOffspring,
        ModelVariant::UngatedMotion,
    ] {
        assert!(!plain.contact());
    }
    first_tick_differs_only_by_contact(
        FIELD_ECOLOGY_V1_CHEMISTRY_ONLY,
        FIELD_ECOLOGY_V1_CONTACT_CHEMISTRY_ONLY,
    );
    first_tick_differs_only_by_contact(
        FIELD_ECOLOGY_V1_FRESH_OFFSPRING,
        FIELD_ECOLOGY_V1_CONTACT_FRESH_OFFSPRING,
    );
}

#[test]
fn a_contact_world_needs_a_positive_body_diameter() {
    let config = ecology(FIELD_ECOLOGY_V1_CONTACT);
    let text = config.canonical_toml();
    let zero = text.replace(
        "child_position_offset_radius = 0.6",
        "child_position_offset_radius = 0.0",
    );
    assert_ne!(text, zero, "the key is in the canonical text");
    let config = ValidatedConfig::from_toml(&zero).expect("a valid configuration");
    let error = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect_err("no body, no contact");
    assert!(
        format!("{error}").contains("child_position_offset_radius"),
        "{error}"
    );
}
