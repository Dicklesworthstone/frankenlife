//! Evidence for the contact models (plan §5.4's "direct collisions … define
//! later model variants"): `field-ecology-v1-contact` adds exactly one rule
//! to `field-ecology-v1`, and its two controls add it to theirs.
//!
//! The oracle is the rule written out over every pair (`flife_engine::contact`
//! sums the same terms over neighbour bins): each survivor of a tick moves by
//! `Σ_j p_ij u_ij` over the other survivors, pushed apart by `(k/2)(D − d)`
//! when closer than the body diameter `D`, and pulled together by
//! `(k_a/2) cos₊(a_i, a_j)² (d − D)(R D − d) / ((R − 1) D)` when within the
//! adhesion reach `R D`; the sum is clamped to `min(D/2, h/2)`. Each contact
//! world is compared with its model without contact from the same founding.
//! On the first tick the two must spend, emit and give birth identically,
//! keep every velocity, and differ only by that displacement of the
//! survivors.

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
use flife_engine::contact::unit_affinity;
use flife_engine::{Arrangement, ModelVariant, NumericalProfile, Stepped, TickReport, World};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// The world side and grid, in world units and cells.
const SIDE: f64 = 16.0;
/// Field cells a side.
const CELLS: f64 = 16.0;
/// The body diameter, `[contact] diameter`.
const DIAMETER: f64 = 0.6;
/// `[contact] stiffness`.
const STIFFNESS: f64 = 0.5;
/// `[contact] adhesion_range`.
const RANGE: f64 = 1.5;

/// The `[contact]` table with `adhesion`.
fn table(adhesion: f64) -> String {
    format!(
        "[contact]\ndiameter = {DIAMETER:?}\nstiffness = {STIFFNESS:?}\nadhesion = {adhesion:?}\nadhesion_range = {RANGE:?}\n"
    )
}

/// Appendix A's starter at 16 × 16 with 640 founders under `model`: crowded
/// enough that uniform founders overlap. A contact model gets [`table`] with
/// `adhesion`.
fn ecology(model: &str, adhesion: f64) -> ValidatedConfig {
    ValidatedConfig::from_toml(&text(model, adhesion)).expect("a valid configuration")
}

fn text(model: &str, adhesion: f64) -> String {
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
    let mut text = lines.join("\n");
    if ModelVariant::of(model).expect("a model").contact() {
        text.push('\n');
        text.push_str(&table(adhesion));
    }
    text
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

/// Each live organism's unit affinity, by identity.
fn affinities(world: &World) -> BTreeMap<u64, [f32; 32]> {
    world
        .population()
        .iter()
        .map(|organism| {
            let genome = world.population().genome(organism.id).expect("a genome");
            (organism.id.get(), unit_affinity(genome.traits().affinity()))
        })
        .collect()
}

fn alike(a: &[f32; 32], b: &[f32; 32]) -> f64 {
    let cosine: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| f64::from(*x) * f64::from(*y))
        .sum();
    cosine.max(0.0)
}

/// The rule over every pair of `survivors` (identity → settled position),
/// with adhesion `adhesion` between their `affinities`.
fn oracle(
    survivors: &BTreeMap<u64, [f64; 2]>,
    affinities: &BTreeMap<u64, [f32; 32]>,
    adhesion: f64,
) -> BTreeMap<u64, [f64; 2]> {
    let limit = (0.5 * DIAMETER).min(0.5 * SIDE / CELLS);
    let reach = RANGE * DIAMETER;
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
                let push = if distance < DIAMETER {
                    0.5 * STIFFNESS * (DIAMETER - distance)
                } else if adhesion > 0.0 && distance < reach {
                    let alike = alike(&affinities[&id], &affinities[&other]);
                    -0.5 * adhesion * alike * alike * (distance - DIAMETER) * (reach - distance)
                        / ((RANGE - 1.0) * DIAMETER)
                } else {
                    continue;
                };
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

/// One tick of `plain` and of `contact` (with `adhesion`) from one founding:
/// the same flows, births, energies and velocities, and positions apart by
/// exactly the oracle's displacement of the survivors.
fn first_tick_differs_only_by_contact(plain: &str, contact: &str, adhesion: f64) {
    let mut without = found(&ecology(plain, 0.0));
    let mut with = found(&ecology(contact, adhesion));
    assert_eq!(state(&without), state(&with), "one founding");
    let chemistry = affinities(&with);
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
    let displacement = oracle(&survivors, &chemistry, adhesion);
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
    first_tick_differs_only_by_contact(FIELD_ECOLOGY_V1, FIELD_ECOLOGY_V1_CONTACT, 0.0);
    first_tick_differs_only_by_contact(FIELD_ECOLOGY_V1, FIELD_ECOLOGY_V1_CONTACT, 0.4);

    // Over 64 ticks the contact world's bodies overlap less than half as much
    // (about a third here), and its ledger closes every tick.
    let mut without = found(&ecology(FIELD_ECOLOGY_V1, 0.0));
    let mut with = found(&ecology(FIELD_ECOLOGY_V1_CONTACT, 0.0));
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
        0.4,
    );
    first_tick_differs_only_by_contact(
        FIELD_ECOLOGY_V1_FRESH_OFFSPRING,
        FIELD_ECOLOGY_V1_CONTACT_FRESH_OFFSPRING,
        0.4,
    );
}

/// The mean `cos₊(a_i, a_j)` over the live pairs within the adhesion reach.
fn neighbours_alike(world: &World) -> f64 {
    let chemistry = affinities(world);
    let bodies: Vec<([f64; 2], u64)> = world
        .population()
        .iter()
        .map(|o| (o.position, o.id.get()))
        .collect();
    let (mut total, mut pairs) = (0.0, 0_u32);
    for (index, &([x, y], id)) in bodies.iter().enumerate() {
        for &([ox, oy], other) in &bodies[index + 1..] {
            if image(x - ox).hypot(image(y - oy)) < RANGE * DIAMETER {
                total += alike(&chemistry[&id], &chemistry[&other]);
                pairs += 1;
            }
        }
    }
    total / f64::from(pairs.max(1))
}

#[test]
fn like_bodies_adhere_and_sort_into_their_own_neighbourhoods() {
    // Adhesion pulls alike bodies together, so over 96 ticks a body's
    // neighbours come to share its chemistry more than without adhesion.
    let mut apart = found(&ecology(FIELD_ECOLOGY_V1_CONTACT, 0.0));
    let mut sticky = found(&ecology(FIELD_ECOLOGY_V1_CONTACT, 1.0));
    for _ in 0..96 {
        publish(&mut apart);
        publish(&mut sticky);
    }
    let (plain, sorted) = (neighbours_alike(&apart), neighbours_alike(&sticky));
    assert!(
        sorted > plain,
        "neighbours alike {sorted} with adhesion, {plain} without"
    );
}

#[test]
fn the_contact_table_belongs_to_the_contact_models() {
    let contact = text(FIELD_ECOLOGY_V1_CONTACT, 0.4);
    assert!(ValidatedConfig::from_toml(&contact).is_ok());
    let refused = |text: &str, path: &str| {
        let error = ValidatedConfig::from_toml(text).expect_err("refused");
        assert!(format!("{error:?}").contains(path), "{error:?}");
    };
    // A contact model without its table; another model with one.
    refused(&contact.replace(&table(0.4), ""), "contact");
    refused(
        &format!("{}\n{}", text(FIELD_ECOLOGY_V1, 0.0), table(0.4)),
        "contact",
    );
    // Out of bounds: no body, no stiffness, adhesion over one, reach inside
    // the body.
    for (from, to, path) in [
        ("diameter = 0.6", "diameter = 0.0", "contact.diameter"),
        ("stiffness = 0.5", "stiffness = 0.0", "contact.stiffness"),
        ("adhesion = 0.4", "adhesion = 1.5", "contact.adhesion"),
        (
            "adhesion_range = 1.5",
            "adhesion_range = 0.9",
            "contact.adhesion_range",
        ),
    ] {
        assert!(contact.contains(from), "{from}");
        refused(&contact.replace(from, to), path);
    }
    // Without the table nothing about another model's configuration
    // changes: its canonical text has no contact key.
    assert!(
        !ecology(FIELD_ECOLOGY_V1, 0.0)
            .canonical_toml()
            .contains("contact")
    );
    assert!(
        ecology(FIELD_ECOLOGY_V1_CONTACT, 0.4)
            .canonical_toml()
            .contains("[contact]\ndiameter = 0.6")
    );
}
