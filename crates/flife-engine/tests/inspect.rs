//! FL-021 evidence (plan §14.2, §21.2 INV-09): an organism snapshot is that
//! organism's committed row and its own genome at the world's tick, its held
//! output decoded as the world decodes it, and taking one changes nothing.
//!
//! The oracle reads the same world through its public accessors
//! (`Population::get`, `Population::genome`, the configuration's topology
//! catalog and action decoding), not through the snapshot. What the field
//! offers an organism where it stands is checked against the plan's
//! footprint, composition, efficiency and request laws written out here
//! over the committed field's planes. A weight dump is checked against the
//! organism's own genome, split by its catalog topology's widths, and its
//! layers evaluated by the declared arithmetic written out here against the
//! reference controller.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use flife_core::ValidatedConfig;
use flife_core::ids::OrganismId;
use flife_core::model::FIELD_ECOLOGY_V1_CHEMISTRY_ONLY;
use flife_core::numerics::tanh;
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_field::FieldState;
use flife_genome::TopologyCatalog;
use flife_neural::{
    ACTION_WIDTH, ActionDecoding, ControllerScratch, ReferenceController, SENSOR_WIDTH,
    WeightDtype, decode, widen,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// Appendix A at 32 × 32 world units with 48 founders and FL-013's small
/// ecology (four richer sources, cheaper upkeep, readier reproduction), so
/// it breeds within tens of ticks, under `model`.
fn small(model: &str) -> ValidatedConfig {
    configured(model, &[])
}

/// [`small`] with `extra` overrides `(section, key, value)`.
fn configured(model: &str, extra: &[(&str, &str, &str)]) -> ValidatedConfig {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    for (section, key, value) in [
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
    ]
    .into_iter()
    .chain(extra.iter().copied())
    {
        let header = format!("[{section}]");
        let at = lines
            .iter()
            .position(|line| line.trim() == header)
            .unwrap_or_else(|| panic!("no section {header}"));
        let line = lines[at + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| at + 1 + offset)
            .unwrap_or_else(|| panic!("no key {key} in {header}"));
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
    .expect("founds")
}

fn step(world: &mut World) {
    assert!(matches!(
        world.step().expect("steps"),
        Stepped::Published(_)
    ));
}

/// Every live organism's snapshot against the world's public accessors.
fn check_every_snapshot(world: &World, config: &ValidatedConfig, chemistry_only: bool) {
    let catalog = TopologyCatalog::from_config(config).expect("catalog");
    let decoding = ActionDecoding::from_config(config).expect("decoding");
    let population = world.population();
    let mut held = 0;
    for id in population.ids() {
        let snapshot = world
            .organism_snapshot(id)
            .expect("inspects")
            .expect("a live organism");
        let organism = population.get(id).expect("live");
        let genome = population.genome(id).expect("its genome");
        assert_eq!(snapshot.tick, world.tick());
        assert_eq!(snapshot.organism, organism);
        assert_eq!(
            snapshot.age_ticks,
            world.tick().get() - organism.birth_tick.get()
        );
        assert_eq!(snapshot.topology, genome.topology().index());
        assert_eq!(
            snapshot.hidden_widths,
            catalog
                .topology(genome.topology())
                .expect("in the catalog")
                .hidden_widths()
        );
        assert_eq!(snapshot.parameters, genome.parameters().len());
        assert_eq!(snapshot.traits, *genome.traits());
        let expected = organism.held.outputs().map(|outputs| {
            let raw = if chemistry_only {
                [0.0; ACTION_WIDTH]
            } else {
                widen(outputs)
            };
            decode(&raw, world.step_size(), &decoding).expect("decodes")
        });
        assert_eq!(snapshot.decoded, expected);
        held += usize::from(snapshot.decoded.is_some());
    }
    assert!(held > 0, "some organism holds an output");
}

#[test]
fn an_organism_snapshot_is_its_committed_row_and_genome() {
    let config = small(flife_core::model::FIELD_ECOLOGY_V1);
    let mut world = found(&config);
    // Until children born at two different ticks live beside founders.
    let births = |world: &World| {
        world
            .population()
            .iter()
            .filter(|organism| organism.parent.is_some())
            .map(|organism| organism.birth_tick)
            .collect::<std::collections::BTreeSet<_>>()
    };
    while births(&world).len() < 2 {
        assert!(world.tick().get() < 400, "no births by tick 400");
        step(&mut world);
    }
    for _ in 0..3 {
        step(&mut world);
    }
    check_every_snapshot(&world, &config, false);
    let tick = world.tick().get();
    let children: Vec<_> = world
        .population()
        .iter()
        .filter(|organism| organism.parent.is_some())
        .collect();
    assert!(!children.is_empty());
    for child in children {
        let snapshot = world
            .organism_snapshot(child.id)
            .expect("inspects")
            .expect("live");
        assert!(child.birth_tick.get() > 0);
        assert_eq!(snapshot.age_ticks, tick - child.birth_tick.get());
        assert!(snapshot.age_ticks < tick);
    }
    // An identity the population does not hold: none.
    assert_eq!(world.organism_snapshot(OrganismId::new(1 << 40)), Ok(None));
    // Founders were born at tick 0, so their age is the tick.
    let founder = world
        .population()
        .iter()
        .find(|organism| organism.parent.is_none())
        .expect("a founder");
    let snapshot = world
        .organism_snapshot(founder.id)
        .expect("inspects")
        .expect("live");
    assert_eq!(snapshot.age_ticks, tick);
    assert_eq!(snapshot.traits.affinity().len(), 32);
}

/// The chemistry-only ablation decodes every held output from zero, and so
/// does its snapshot.
#[test]
fn a_chemistry_only_snapshot_decodes_from_zero() {
    let config = small(FIELD_ECOLOGY_V1_CHEMISTRY_ONLY);
    let mut world = found(&config);
    for _ in 0..6 {
        step(&mut world);
    }
    check_every_snapshot(&world, &config, true);
}

/// INV-09: two worlds from one founding, one inspected in full before and
/// after every tick, commit the same digests.
#[test]
fn inspecting_changes_nothing() {
    let config = small(flife_core::model::FIELD_ECOLOGY_V1);
    let mut inspected = found(&config);
    let mut plain = found(&config);
    for _ in 0..20 {
        let before = inspected.digest();
        let ids: Vec<OrganismId> = inspected.population().ids().collect();
        for id in ids {
            assert!(inspected.organism_snapshot(id).expect("inspects").is_some());
        }
        assert_eq!(
            inspected.organism_snapshot(OrganismId::new(1 << 40)),
            Ok(None)
        );
        assert_eq!(inspected.digest(), before);
        step(&mut inspected);
        step(&mut plain);
        assert_eq!(inspected.digest(), plain.digest());
        assert_eq!(inspected.tick(), plain.tick());
    }
}

/// The `grid-cic-v1` footprint at `[x, y]` written out from §4.3: the
/// continuous index `(x/h_x − ½, y/h_y − ½)`; the cell at its floor and the
/// three to its right and above, wrapped onto the torus and weighted
/// `(1−f_x)(1−f_y)`, `f_x(1−f_y)`, `(1−f_x)f_y` and `f_x f_y`; a cell named
/// twice merged and a zero weight dropped; sorted by row-major index. Also
/// whether it wrapped.
fn cic(config: &ValidatedConfig, [x, y]: [f64; 2]) -> (Vec<(usize, f64)>, bool) {
    let world = &config.config().world;
    let (columns, rows) = (i64::from(world.grid_width), i64::from(world.grid_height));
    let u = x.rem_euclid(world.width) / (world.width / f64::from(world.grid_width)) - 0.5;
    let v = y.rem_euclid(world.height) / (world.height / f64::from(world.grid_height)) - 0.5;
    let (fx, fy) = (u - u.floor(), v - v.floor());
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a floor of a continuous index on a 32-cell side"
    )]
    let (column, row) = (u.floor() as i64, v.floor() as i64);
    let mut cells: Vec<(usize, f64)> = Vec::new();
    let mut wrapped = false;
    for (dx, dy, weight) in [
        (0, 0, (1.0 - fx) * (1.0 - fy)),
        (1, 0, fx * (1.0 - fy)),
        (0, 1, (1.0 - fx) * fy),
        (1, 1, fx * fy),
    ] {
        if weight == 0.0 {
            continue;
        }
        let (cx, cy) = (column + dx, row + dy);
        wrapped |= !(0..columns).contains(&cx) || !(0..rows).contains(&cy);
        let index = cy.rem_euclid(rows) * columns + cx.rem_euclid(columns);
        let index = usize::try_from(index).expect("on the grid");
        match cells.iter_mut().find(|(cell, _)| *cell == index) {
            Some((_, merged)) => *merged += weight,
            None => cells.push((index, weight)),
        }
    }
    cells.sort_by_key(|&(cell, _)| cell);
    (cells, wrapped)
}

/// §5.3's `η = η_max tanh(k a·z − b)` at `cell`, with C.3's composition
/// `z = m/e` from the field's F32 planes (neutral for an empty cell) and
/// the standard library's `tanh`.
fn efficiency(
    config: &ValidatedConfig,
    affinity: &[f64; 32],
    field: &FieldState,
    cell: usize,
) -> f64 {
    let chemistry = &config.config().chemistry;
    let energy = f64::from(field.energy()[cell]);
    let mut dot = 0.0;
    if energy > 0.0 {
        for (channel, a) in (0_u32..).zip(affinity) {
            let moment = f64::from(field.moment(channel).expect("a moment plane")[cell]);
            dot += a * (moment / energy);
        }
    }
    chemistry.maximum_efficiency
        * (chemistry.affinity_gain * dot - chemistry.discrimination_threshold).tanh()
}

/// Plan §14.2's Chemistry: every organism's snapshot holds its exchange
/// footprint on the committed field, each footprint cell's energy and
/// efficiency for its affinity, the gain and toxicity per unit taken in the
/// footprint's proportions, and A.1's request at the footprint's density.
/// The oracle is §4.3, C.3, §5.3 and A.1 written out over the field's
/// planes and the configuration, not the engine's functions. The footprint's
/// cells, weights and energies are exact; the rest, after `tanh`, within
/// 1e-12. On the small world's unit cells and on 2 × 2 cells, so a density
/// is an amount over the cell's area.
#[test]
fn a_snapshot_holds_what_the_committed_field_offers_where_it_stands() {
    for config in [
        small(flife_core::model::FIELD_ECOLOGY_V1),
        configured(
            flife_core::model::FIELD_ECOLOGY_V1,
            &[
                ("world", "grid_width", "16"),
                ("world", "grid_height", "16"),
                // Sensing no finer than a cell.
                ("sensors", "minimum_scale_world_units", "2.0"),
                ("sensors", "initial_scale_world_units", "2.0"),
            ],
        ),
    ] {
        local_chemistry_against_the_laws(&config);
    }
}

/// [`a_snapshot_holds_what_the_committed_field_offers_where_it_stands`] on
/// `config`.
fn local_chemistry_against_the_laws(config: &ValidatedConfig) {
    let chemistry = &config.config().chemistry;
    let shape = &config.config().world;
    let area =
        (shape.width / f64::from(shape.grid_width)) * (shape.height / f64::from(shape.grid_height));
    let seconds = f64::from(shape.tick_numerator) / f64::from(shape.tick_denominator);
    let close = |got: f64, want: f64| (got - want).abs() <= 1e-12 * want.abs().max(1.0);
    let mut world = found(config);
    let (mut useful, mut harmful, mut wrapped, mut full, mut read) = (0, 0, 0, 0, 0);
    for _ in 0..12 {
        step(&mut world);
        let field = world.field();
        for organism in world.population().iter() {
            let snapshot = world
                .organism_snapshot(organism.id)
                .expect("inspects")
                .expect("live");
            let local = &snapshot.local;
            let (footprint, wraps) = cic(config, organism.position);
            assert_eq!(
                local
                    .cells
                    .iter()
                    .map(|cell| (cell.cell, cell.weight.to_bits()))
                    .collect::<Vec<_>>(),
                footprint
                    .iter()
                    .map(|&(cell, weight)| (cell, weight.to_bits()))
                    .collect::<Vec<_>>(),
                "{} at {:?}",
                organism.id,
                organism.position
            );
            let (mut density, mut gain, mut harm) = (0.0, 0.0, 0.0);
            for (cell, &(index, weight)) in local.cells.iter().zip(&footprint) {
                assert_eq!(cell.energy.to_bits(), field.energy()[index].to_bits());
                let eta = efficiency(config, snapshot.traits.affinity(), field, index);
                assert!(
                    close(cell.efficiency, eta),
                    "{} at cell {index}: η {} against {eta}",
                    organism.id,
                    cell.efficiency
                );
                density += weight * f64::from(field.energy()[index]);
                gain += weight * eta.max(0.0);
                harm += weight * (-eta).max(0.0);
                useful += usize::from(eta > 0.0);
                harmful += usize::from(eta < 0.0);
            }
            density /= area;
            let request = seconds * chemistry.uptake_capacity_per_second * density
                / (chemistry.uptake_half_saturation_density + density);
            assert!(close(local.density, density), "{} {density}", local.density);
            assert!(close(local.request, request), "{} {request}", local.request);
            assert!(close(local.gain, gain), "{} {gain}", local.gain);
            assert!(
                close(local.toxicity, chemistry.toxicity_sensitivity * harm),
                "{} {harm}",
                local.toxicity
            );
            wrapped += usize::from(wraps);
            full += usize::from(footprint.len() == 4);
            read += 1;
        }
    }
    assert!(
        useful > 0 && harmful > 0 && wrapped > 0 && full > 0,
        "{area} world-length² cells, {read} footprints: {useful} useful cells, {harmful} harmful, \
         {wrapped} wrapped, {full} of four cells"
    );
}

/// The declared F32 arithmetic written out (flife-neural's controller
/// module) over each `(fan_in, weights, biases)` layer in evaluation order:
/// each input rounded once to F32; each output the bias, then each input's
/// product in ascending index, every product and sum in F32; `tanh` in F64
/// rounded once after each hidden layer; the last layer linear.
fn forward(layers: &[(usize, &[f32], &[f32])], inputs: &[f64; SENSOR_WIDTH]) -> Vec<f32> {
    let mut source: Vec<f32> = inputs.iter().map(|&input| input as f32).collect();
    for (index, &(fan_in, weights, biases)) in layers.iter().enumerate() {
        assert_eq!(fan_in, source.len());
        assert_eq!(weights.len(), fan_in * biases.len());
        let z: Vec<f32> = biases
            .iter()
            .zip(weights.chunks_exact(fan_in))
            .map(|(&bias, row)| {
                let mut z = bias;
                for (weight, input) in row.iter().zip(&source) {
                    z += weight * input;
                }
                z
            })
            .collect();
        if index + 1 == layers.len() {
            return z;
        }
        source = z
            .iter()
            .map(|&z| tanh(f64::from(z)).expect("finite") as f32)
            .collect();
    }
    panic!("no layers")
}

fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|value| value.to_bits()).collect()
}

/// Plan §14.2's on-demand weight dump: every organism's dump, after
/// children of two birth ticks live beside the founders, is its own genome's
/// parameters whole and bit for bit, split by its catalog topology's widths
/// (104 inputs, its hidden widths, 36 outputs) into consecutive layers of
/// `weight[out][in]` then `bias[out]` covering every parameter. Its layers,
/// evaluated by the declared arithmetic written out here, give the
/// reference controller's outputs bit for bit on three input vectors. An
/// unknown identity gives `None`, and dumping changes nothing (INV-09).
#[test]
fn a_weight_dump_is_the_organisms_own_controller() {
    let config = small(flife_core::model::FIELD_ECOLOGY_V1);
    let catalog = TopologyCatalog::from_config(&config).expect("catalog");
    let mut world = found(&config);
    let births = |world: &World| {
        world
            .population()
            .iter()
            .filter(|organism| organism.parent.is_some())
            .map(|organism| organism.birth_tick)
            .collect::<std::collections::BTreeSet<_>>()
    };
    while births(&world).len() < 2 {
        assert!(world.tick().get() < 400, "no births by tick 400");
        step(&mut world);
    }
    let before = world.digest();
    let inputs: [[f64; SENSOR_WIDTH]; 3] = [
        [0.0; SENSOR_WIDTH],
        std::array::from_fn(|index| (index as f64 * 0.37).sin()),
        std::array::from_fn(|index| if index % 2 == 0 { 1.0 } else { -0.5 }),
    ];
    let population = world.population();
    let mut dumped = 0;
    for organism in population.iter() {
        let dump = world
            .organism_weights(organism.id)
            .expect("dumps")
            .expect("a live organism");
        let genome = population.genome(organism.id).expect("its genome");
        assert_eq!(dump.tick, world.tick());
        assert_eq!(dump.organism, organism.id);
        assert_eq!(dump.genome, organism.genome);
        assert_eq!(dump.topology, genome.topology().index());
        let parameters = genome.parameters().as_f32().expect("an F32 genome");
        assert_eq!(dump.dtype, WeightDtype::F32);
        assert_eq!(bits(&dump.parameters), bits(parameters));
        let topology = catalog.topology(genome.topology()).expect("in the catalog");
        let widths: Vec<usize> = std::iter::once(SENSOR_WIDTH)
            .chain(topology.hidden_widths().iter().map(|&width| width as usize))
            .chain(std::iter::once(ACTION_WIDTH))
            .collect();
        assert_eq!(dump.layers.len(), widths.len() - 1);
        let mut at = 0;
        let mut layers = Vec::new();
        for (index, (shape, pair)) in dump.layers.iter().zip(widths.windows(2)).enumerate() {
            let (fan_in, fan_out) = (pair[0], pair[1]);
            assert_eq!((shape.inputs, shape.outputs), (fan_in, fan_out));
            let (weights, biases) = dump.layer(index).expect("a layer");
            assert_eq!(bits(weights), bits(&parameters[at..at + fan_in * fan_out]));
            at += fan_in * fan_out;
            assert_eq!(bits(biases), bits(&parameters[at..at + fan_out]));
            at += fan_out;
            layers.push((fan_in, weights, biases));
        }
        assert_eq!(at, parameters.len(), "every parameter in a layer");
        assert_eq!(dump.layer(dump.layers.len()), None);
        let reference =
            ReferenceController::sealed(topology, genome.parameters()).expect("a controller");
        let mut scratch = ControllerScratch::default();
        for inputs in &inputs {
            let expected = reference.evaluate(inputs, &mut scratch).expect("evaluates");
            assert_eq!(bits(&forward(&layers, inputs)), bits(&expected));
        }
        dumped += 1;
    }
    assert!(dumped > 48, "{dumped} organisms, children among them");
    assert_eq!(world.organism_weights(OrganismId::new(1 << 40)), Ok(None));
    assert_eq!(world.digest(), before);
}
