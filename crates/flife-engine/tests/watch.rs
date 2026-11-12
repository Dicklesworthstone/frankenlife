//! FL-021 evidence (plan §14.2's Inputs and Brain, §21.2 INV-09): a watched
//! organism's snapshot carries the observation its decision read in the
//! newest published step, and the hidden activations of the evaluation its
//! held output came from; watching changes nothing a run computes.
//!
//! The oracles are not the snapshot's path:
//! - the inputs: a controller backend that records every job's inputs and
//!   defers every job to the reference, so the run stays the reference's;
//! - the observation: `observe` of the recorded raw sample and the
//!   organism's committed row at the step's start;
//! - the activations: the declared F32 arithmetic (flife-neural's controller
//!   module: the bias, then each input's product in ascending index, all in
//!   F32; `tanh` in F64 rounded once), written out here over the genome's
//!   parameters, which must give the reported activations and the held
//!   output bit for bit.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::sync::{Arc, Mutex};

use flife_core::clock::age_seconds;
use flife_core::ids::{GenomeId, OrganismId};
use flife_core::model::FIELD_ECOLOGY_V1;
use flife_core::numerics::tanh;
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::{
    Advance, Arrangement, NumericalProfile, OrganismSnapshot, Stepped, TickReport, World,
};
use flife_neural::{
    ACTION_WIDTH, ControllerBackend, ControllerJob, ControllerScratch, Intrinsic, SENSOR_WIDTH,
    SensorScales, observe,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// Appendix A at 32 × 32 world units with 48 founders and FL-013's small
/// ecology (four richer sources, cheaper upkeep, readier reproduction), so
/// it breeds within tens of ticks, a controller evaluated every `interval`
/// ticks.
fn small(interval: u32) -> ValidatedConfig {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    let interval = interval.to_string();
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
        ("controller", "evaluation_interval_ticks", interval.as_str()),
    ] {
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
    lines[model_line] = format!("model = \"{FIELD_ECOLOGY_V1}\"");
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

fn step(world: &mut World) -> TickReport {
    match world.step().expect("steps") {
        Stepped::Published(report) => *report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

fn snapshot(world: &World, id: OrganismId) -> OrganismSnapshot {
    world
        .organism_snapshot(id)
        .expect("inspects")
        .expect("a live organism")
}

/// Every job's genome and inputs in the newest call.
type Jobs = Arc<Mutex<Vec<(GenomeId, [f64; SENSOR_WIDTH])>>>;

/// Records every job and defers it, so the reference evaluates it.
#[derive(Debug)]
struct Recorder {
    jobs: Jobs,
}

impl ControllerBackend for Recorder {
    fn describe(&self) -> String {
        "recorder (test)".to_owned()
    }

    fn resident_bytes(&self) -> u64 {
        0
    }

    fn forget(&mut self) {}

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
    ) -> Result<(), CoreError> {
        let mut recorded = self.jobs.lock().expect("not poisoned");
        recorded.clear();
        outputs.clear();
        for job in jobs {
            recorded.push((job.genome, *job.inputs));
            outputs.push(None);
        }
        Ok(())
    }
}

/// A backend that breaks its contract: the reference's outputs, the first
/// of each with its lowest bit flipped.
#[derive(Debug, Default)]
struct Liar {
    scratch: ControllerScratch,
}

impl ControllerBackend for Liar {
    fn describe(&self) -> String {
        "liar (test)".to_owned()
    }

    fn resident_bytes(&self) -> u64 {
        0
    }

    fn forget(&mut self) {}

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
    ) -> Result<(), CoreError> {
        outputs.clear();
        for job in jobs {
            let mut raw = job.controller.evaluate(job.inputs, &mut self.scratch)?;
            raw[0] = f32::from_bits(raw[0].to_bits() ^ 1);
            outputs.push(Some(raw));
        }
        Ok(())
    }
}

fn recording(world: &mut World) -> Jobs {
    let jobs = Jobs::default();
    world.set_controller_backend(Some(Box::new(Recorder {
        jobs: Arc::clone(&jobs),
    })));
    jobs
}

/// The inputs `genome`'s job read in the newest call.
fn job_inputs(jobs: &Jobs, genome: GenomeId) -> Option<[f64; SENSOR_WIDTH]> {
    jobs.lock()
        .expect("not poisoned")
        .iter()
        .find(|(of, _)| *of == genome)
        .map(|(_, inputs)| *inputs)
}

/// The declared arithmetic over `parameters` laid out as `widths` (inputs,
/// hidden widths, outputs): each hidden layer's activations, then the raw
/// outputs.
fn forward(
    parameters: &[f32],
    widths: &[usize],
    inputs: &[f64; SENSOR_WIDTH],
) -> (Vec<Vec<f32>>, Vec<f32>) {
    let mut source: Vec<f32> = inputs.iter().map(|&input| input as f32).collect();
    let mut at = 0;
    let mut hidden = Vec::new();
    for (layer, pair) in widths.windows(2).enumerate() {
        let (fan_in, fan_out) = (pair[0], pair[1]);
        let weights = &parameters[at..at + fan_in * fan_out];
        let biases = &parameters[at + fan_in * fan_out..at + fan_in * fan_out + fan_out];
        at += fan_in * fan_out + fan_out;
        let z: Vec<f32> = (0..fan_out)
            .map(|output| {
                let mut z = biases[output];
                for (weight, input) in weights[output * fan_in..(output + 1) * fan_in]
                    .iter()
                    .zip(&source)
                {
                    z += weight * input;
                }
                z
            })
            .collect();
        if layer + 2 == widths.len() {
            assert_eq!(at, parameters.len(), "every parameter read");
            return (hidden, z);
        }
        let activations: Vec<f32> = z
            .iter()
            .map(|&z| tanh(f64::from(z)).expect("finite") as f32)
            .collect();
        hidden.push(activations.clone());
        source = activations;
    }
    panic!("no output layer")
}

fn bits64(values: &[f64]) -> Vec<u64> {
    values.iter().map(|value| value.to_bits()).collect()
}

fn bits32(values: &[f32]) -> Vec<u32> {
    values.iter().map(|value| value.to_bits()).collect()
}

/// Its activations and held output against [`forward`] on `inputs`.
fn check_activations(world: &World, id: OrganismId, inputs: &[f64; SENSOR_WIDTH], at: Tick) {
    let snapshot = snapshot(world, id);
    let activations = snapshot.activations.as_ref().expect("activations");
    assert_eq!(activations.tick, at);
    let organism = world.population().get(id).expect("lives");
    assert_eq!(organism.held.evaluated_at(), Some(at));
    let genome = world.population().genome(id).expect("its genome");
    let widths: Vec<usize> = std::iter::once(SENSOR_WIDTH)
        .chain(snapshot.hidden_widths.iter().map(|&width| width as usize))
        .chain(std::iter::once(ACTION_WIDTH))
        .collect();
    let (hidden, outputs) = forward(genome.parameters(), &widths, inputs);
    assert_eq!(activations.hidden.len(), snapshot.hidden_widths.len());
    for (reported, expected) in activations.hidden.iter().zip(&hidden) {
        assert_eq!(bits32(reported), bits32(expected));
    }
    assert_eq!(
        bits32(&outputs),
        bits32(organism.held.outputs().expect("held")),
        "the activations give the held output"
    );
}

/// Each step's record is the observation its controller job read, the
/// encoding of the recorded raw sample and its row at the step's start, and
/// the activations the declared arithmetic gives on it, which give the held
/// output. No other organism's snapshot records anything.
#[test]
fn a_watched_snapshot_holds_what_its_controller_read_and_its_activations() {
    let config = small(1);
    let mut world = found(&config);
    let jobs = recording(&mut world);
    let scales = SensorScales::from_config(&config).expect("scales");
    let id = world.population().ids().next().expect("a founder");
    assert_eq!(world.watched(), None);
    world.watch(Some(id));
    assert_eq!(world.watched(), Some(id));
    // Nothing is recorded before the first watched step.
    let before = snapshot(&world, id);
    assert_eq!((before.sensed, before.activations), (None, None));
    let mut births = 0;
    for _ in 0..48 {
        let row = world.population().get(id).expect("lives");
        let from = world.tick();
        births += step(&mut world).births.len();
        let snapshot = snapshot(&world, id);
        let sensed = snapshot.sensed.as_ref().expect("recorded");
        assert_eq!(sensed.tick, from);
        assert!(sensed.evaluated, "every organism evaluates every tick");
        assert_eq!(
            sensed.scale.to_bits(),
            snapshot.traits.sensing_scale().to_bits()
        );
        let inputs = job_inputs(&jobs, row.genome).expect("its job");
        assert_eq!(bits64(&sensed.observation.inputs), bits64(&inputs));
        let intrinsic = Intrinsic {
            energy: row.energy,
            age_seconds: age_seconds(row.birth_tick, from, world.step_size()).expect("an age"),
            velocity: row.velocity,
            energy_change_per_second: row.energy_change_rate,
        };
        let encoded = observe(&sensed.raw, &intrinsic, &scales).expect("encodes");
        assert_eq!(bits64(&encoded.inputs), bits64(&inputs));
        assert_eq!(encoded.gradients, sensed.observation.gradients);
        check_activations(&world, id, &inputs, world.tick());
        for other in world.population().ids().filter(|&other| other != id) {
            let unwatched = self::snapshot(&world, other);
            assert_eq!((unwatched.sensed, unwatched.activations), (None, None));
        }
    }
    assert!(births > 0, "the world bred while watched");
}

/// The records follow the watch: kept when the same organism is watched
/// again, dropped for another, absent from a cancelled candidate, and none
/// made once nothing is watched.
#[test]
fn the_records_follow_the_watch() {
    let config = small(1);
    let mut world = found(&config);
    let ids: Vec<OrganismId> = world.population().ids().take(2).collect();
    let [first, second] = [ids[0], ids[1]];
    world.watch(Some(first));
    step(&mut world);
    step(&mut world);
    let recorded = snapshot(&world, first);
    assert_eq!(
        recorded.sensed.as_ref().map(|sensed| sensed.tick),
        Some(Tick::new(1))
    );
    world.watch(Some(first));
    assert_eq!(snapshot(&world, first), recorded, "watched again: kept");
    match world.advance().expect("advances") {
        Advance::Ready(candidate) => candidate.cancel(),
        Advance::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
    assert_eq!(
        snapshot(&world, first),
        recorded,
        "a cancelled tick records nothing"
    );
    world.watch(Some(second));
    for id in [first, second] {
        let snapshot = snapshot(&world, id);
        assert_eq!((snapshot.sensed, snapshot.activations), (None, None));
    }
    step(&mut world);
    assert!(snapshot(&world, second).sensed.is_some());
    assert!(snapshot(&world, first).sensed.is_none());
    world.watch(None);
    assert_eq!(world.watched(), None);
    step(&mut world);
    for id in [first, second] {
        let snapshot = snapshot(&world, id);
        assert_eq!((snapshot.sensed, snapshot.activations), (None, None));
    }
}

/// At a cadence of three ticks the record of a step it held through says
/// so, and the activations stay those of the evaluation its held output
/// came from: none before its first watched evaluation.
#[test]
fn at_a_slower_cadence_the_activations_are_the_held_outputs() {
    let config = small(3);
    let mut world = found(&config);
    let jobs = recording(&mut world);
    let id = world.population().ids().next().expect("a founder");
    // One step evaluates every founder; then the cadence holds.
    step(&mut world);
    world.watch(Some(id));
    let genome = world.population().get(id).expect("lives").genome;
    let mut evaluated: Option<([f64; SENSOR_WIDTH], Tick)> = None;
    let (mut held_through, mut evaluations) = (0, 0);
    for _ in 0..9 {
        step(&mut world);
        let snapshot = snapshot(&world, id);
        let sensed = snapshot.sensed.as_ref().expect("recorded");
        let due = world
            .population()
            .get(id)
            .expect("lives")
            .held
            .evaluated_at()
            == Some(world.tick());
        assert_eq!(sensed.evaluated, due);
        if due {
            let inputs = job_inputs(&jobs, genome).expect("its job");
            assert_eq!(bits64(&sensed.observation.inputs), bits64(&inputs));
            evaluated = Some((inputs, world.tick()));
            evaluations += 1;
        } else {
            assert!(job_inputs(&jobs, genome).is_none(), "no job while it holds");
            held_through += 1;
        }
        match evaluated {
            Some((inputs, at)) => check_activations(&world, id, &inputs, at),
            None => assert_eq!(snapshot.activations, None),
        }
    }
    assert_eq!((evaluations, held_through), (3, 6));
}

/// INV-09: a world watching one organism after another, inspecting it
/// before every tick, publishes every tick's report and digest as a world
/// that watches nothing, at either cadence.
#[test]
fn watching_changes_nothing_a_run_computes() {
    for interval in [1, 3] {
        let config = small(interval);
        let mut watched = found(&config);
        let mut plain = found(&config);
        let ids: Vec<OrganismId> = watched.population().ids().collect();
        let mut births = 0;
        for tick in 0..40 {
            if tick % 10 == 0 {
                watched.watch(Some(ids[tick / 10 * 7 % ids.len()]));
            }
            let id = watched.watched().expect("watching");
            // It may have died: inspecting is all that matters here.
            let _ = watched.organism_snapshot(id).expect("inspects");
            let report = step(&mut watched);
            births += report.births.len();
            assert_eq!(report, step(&mut plain), "interval {interval}, tick {tick}");
            assert_eq!(watched.digest(), plain.digest());
        }
        assert!(births > 0, "the worlds bred");
    }
}

/// A backend that returns outputs other than the reference's breaks its
/// contract; the world holds what it returned, and the inspector, which
/// evaluates the watched organism again on the reference, refuses the
/// snapshot rather than show activations that did not produce the output
/// held. An unwatched organism's snapshot carries no activations, so it is
/// not checked.
#[test]
fn the_inspector_refuses_an_output_the_reference_does_not_reproduce() {
    let config = small(1);
    let mut world = found(&config);
    world.set_controller_backend(Some(Box::new(Liar::default())));
    let ids: Vec<OrganismId> = world.population().ids().take(2).collect();
    world.watch(Some(ids[0]));
    step(&mut world);
    assert!(matches!(
        world.organism_snapshot(ids[0]),
        Err(CoreError::StateConflict { .. })
    ));
    assert!(world.organism_snapshot(ids[1]).expect("inspects").is_some());
}
