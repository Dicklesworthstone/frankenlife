//! FL-021 evidence (plan §14.2's Inputs, Brain and Transactions, §21.2
//! INV-09): a watched organism's snapshot carries the observation its
//! decision read in the newest published step, the hidden activations of
//! the evaluation its held output came from, and what its budget stages
//! paid and received in its recent steps; watching changes nothing a run
//! computes.
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
//!   output bit for bit;
//! - the transactions: each record replayed on a fresh `EnergyBudget` in
//!   §8.2's stage order, which must pay what it names and leave its closing
//!   energy; the organism's next row (energy, velocity) and the report's
//!   births, rejections and deaths; and the tick's ledger closure, whose
//!   organism flows and transfers must be the records of every organism
//!   summed in the ledger's order, bit for bit, one watching world per
//!   organism.

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
use flife_core::{CompensatedSum, CoreError, EnergyBudget, Flow, Tick, Transfer, ValidatedConfig};
use flife_engine::{
    Advance, Arrangement, EmissionPolicy, MaintenanceSettlement, NumericalProfile, Organism,
    OrganismSnapshot, RECENT_STEPS, ReproductionOutcome, Stepped, TickReport, Transactions, World,
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
    configured(interval, &[])
}

/// [`small`] at costs that starve it: upkeep and motion dear enough that
/// organisms die within tens of ticks, some moving less than they proposed,
/// and a tiny reservoir that takes balances below 0.2.
fn starving() -> ValidatedConfig {
    configured(
        1,
        &[
            ("costs", "basal_energy_per_second", "30.0"),
            ("costs", "motion_energy_per_speed_squared_second", "1.0e8"),
            ("world", "tiny_energy_threshold", "0.2"),
        ],
    )
}

/// [`small`] with almost no mutation: no weight or architecture mutation and
/// chemistry and sensing steps far below canonical rounding, so most
/// candidates are effective no-ops, and a birth whose every candidate is
/// one is rejected (C.16), charging nothing.
fn rejecting() -> ValidatedConfig {
    configured(
        1,
        &[
            ("architecture_mutation", "probability_per_birth", "0.0"),
            ("mutation", "weight_probability", "0.0"),
            ("mutation", "replacement_probability", "0.0"),
            ("mutation", "chemistry_standard_deviation", "1.0e-30"),
            (
                "mutation",
                "sensing_log_scale_standard_deviation",
                "1.0e-30",
            ),
        ],
    )
}

/// [`small`] with `extra` overrides `(section, key, value)`.
fn configured(interval: u32, extra: &[(&str, &str, &str)]) -> ValidatedConfig {
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
    let (hidden, outputs) = forward(
        genome.parameters().as_f32().expect("an F32 genome"),
        &widths,
        inputs,
    );
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
    let ticks: Vec<Tick> = recorded
        .transactions
        .iter()
        .map(|record| record.tick)
        .collect();
    assert_eq!(ticks, [Tick::new(0), Tick::new(1)]);
    assert!(
        world
            .watched_transactions()
            .eq(recorded.transactions.iter())
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
    assert_eq!(world.watched_transactions().len(), 0);
    for id in [first, second] {
        let snapshot = snapshot(&world, id);
        assert_eq!((snapshot.sensed, snapshot.activations), (None, None));
        assert!(snapshot.transactions.is_empty());
    }
    step(&mut world);
    assert!(snapshot(&world, second).sensed.is_some());
    assert_eq!(snapshot(&world, second).transactions.len(), 1);
    assert!(snapshot(&world, first).sensed.is_none());
    assert!(snapshot(&world, first).transactions.is_empty());
    world.watch(None);
    assert_eq!(world.watched(), None);
    step(&mut world);
    assert_eq!(world.watched_transactions().len(), 0);
    for id in [first, second] {
        let snapshot = snapshot(&world, id);
        assert_eq!((snapshot.sensed, snapshot.activations), (None, None));
        assert!(snapshot.transactions.is_empty());
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

/// One term of a maintenance payment.
type Part = fn(&MaintenanceSettlement) -> f64;

/// `values` summed as the ledger sums them.
fn total(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut sum = CompensatedSum::ZERO;
    for value in values {
        sum.add(value);
    }
    sum.value()
}

/// Whether the record's organism made a viable attempt: phase 8 settles
/// those parents' last stages after the other organisms'.
fn attempted(record: &Transactions) -> bool {
    matches!(
        record.reproduction,
        Some(ReproductionOutcome::Born { .. } | ReproductionOutcome::Rejected { .. })
    )
}

/// `record` replayed on a fresh budget from `row`, its organism at the
/// step's start, in §8.2's stage order: the gain credited and the toxicity
/// paid, then maintenance, motion, emission, the parent's share kept and the
/// tiny reservoir. Each payment must pay what the record says, a stage must
/// be recorded exactly when the budget reached it alive, a scaled motion's
/// velocity must be its proposal scaled as Appendix A says, and the balance
/// left must be the closing energy. `rho` is emission's transfer
/// efficiency.
fn replay(record: &Transactions, row: &Organism, rho: f64) {
    assert_eq!(record.opening.to_bits(), row.energy.to_bits(), "opening");
    let mut budget = EnergyBudget::new(record.opening).expect("a budget");
    let uptake = record.uptake;
    if uptake.gain > 0.0 {
        budget.credit(uptake.gain).expect("credits");
    }
    assert_eq!(
        budget.pay(uptake.toxicity.requested).expect("pays"),
        uptake.toxicity
    );
    assert_eq!(record.maintenance.is_some(), !budget.is_exhausted());
    if let Some(maintenance) = record.maintenance {
        assert_eq!(
            budget.pay(maintenance.cost.requested).expect("pays"),
            maintenance.cost
        );
    }
    assert_eq!(record.motion.is_some(), !budget.is_exhausted());
    if let Some(motion) = record.motion {
        let available = budget.remaining();
        if motion.scaled {
            assert!(motion.cost.requested > available);
            // Appendix A: an unaffordable proposal is scaled whole by
            // sqrt(E_available / cost).
            let factor = (available / motion.cost.requested).sqrt();
            assert_eq!(
                bits64(&motion.velocity),
                bits64(&motion.proposed.map(|velocity| velocity * factor))
            );
            assert_eq!(
                budget.pay(available).expect("pays").paid.to_bits(),
                motion.cost.paid.to_bits()
            );
        } else {
            assert!(motion.cost.requested <= available);
            assert_eq!(
                budget.pay(motion.cost.requested).expect("pays"),
                motion.cost
            );
            assert_eq!(bits64(&motion.velocity), bits64(&motion.proposed));
        }
    }
    assert_eq!(record.emission.is_some(), !budget.is_exhausted());
    if let Some(emission) = record.emission {
        assert_eq!(emission.available.to_bits(), budget.remaining().to_bits());
        if emission.cost.requested > 0.0 {
            assert_eq!(
                budget.pay(emission.cost.requested).expect("pays"),
                emission.cost
            );
            assert_eq!(
                emission.offered.to_bits(),
                (rho * emission.cost.requested).to_bits()
            );
        } else {
            assert_eq!(emission.offered, 0.0);
        }
    }
    assert_eq!(record.reproduction.is_some(), !budget.is_exhausted());
    match record.reproduction {
        Some(ReproductionOutcome::Born { split, .. }) => {
            assert_eq!(split.available.to_bits(), budget.remaining().to_bits());
            budget.retain(split.parent).expect("keeps its share");
        }
        Some(ReproductionOutcome::Rejected { split, .. }) => {
            assert_eq!(split.available.to_bits(), budget.remaining().to_bits());
        }
        Some(ReproductionOutcome::CoolingDown { deadline }) => {
            assert_eq!(deadline, row.cooldown_deadline);
            assert!(record.tick < deadline);
        }
        Some(ReproductionOutcome::NoAttempt | ReproductionOutcome::Unviable(_)) | None => {}
    }
    if record.tiny > 0.0 {
        assert_eq!(record.tiny.to_bits(), budget.remaining().to_bits());
        budget.pay(record.tiny).expect("pays");
    }
    assert_eq!(
        record.closing.to_bits(),
        budget.remaining().to_bits(),
        "closing"
    );
}

/// Every live organism has a world watching it, in lockstep with an
/// unwatched reference. Each step, every organism's newest record replays
/// on its budget and agrees with its next row and the report; and the
/// records of all of them, summed in the ledger's order, are the tick's
/// organism flows and transfers bit for bit: toxicity, maintenance, sensor
/// and controller costs, motion, the tiny reservoir, uptake, the child
/// endowments and the emissions paid. The watching worlds' reports are the
/// reference's (INV-09).
#[test]
fn every_organisms_transactions_replay_on_its_budget_and_add_up_to_the_ledger() {
    let [bred, starved, refused] =
        [small(1), starving(), rejecting()].map(|config| transactions_add_up(&config));
    assert!(
        bred.births > 0 && bred.cooling > 0 && bred.emitted > 0,
        "{bred:?}"
    );
    assert!(
        starved.deaths > 0 && starved.scaled > 0 && starved.tiny > 0,
        "{starved:?}"
    );
    assert!(refused.rejected > 0, "{refused:?}");
}

/// What a run of [`transactions_add_up`] exercised, in organism steps.
#[derive(Debug, Default)]
struct Exercised {
    births: usize,
    rejected: usize,
    cooling: usize,
    emitted: usize,
    scaled: usize,
    moving: usize,
    tiny: usize,
    deaths: usize,
    worlds: usize,
}

/// The test above for one configuration, over 40 ticks.
fn transactions_add_up(config: &ValidatedConfig) -> Exercised {
    let rho = EmissionPolicy::from_config(config)
        .expect("emission")
        .efficiency();
    let mut reference = found(config);
    let mut watchers: Vec<World> = Vec::new();
    let mut seen = Exercised::default();
    for tick in 0..40 {
        let from = reference.tick();
        let ids: Vec<OrganismId> = reference.population().ids().collect();
        let rows: Vec<Organism> = ids
            .iter()
            .map(|&id| reference.population().get(id).expect("lives"))
            .collect();
        // A world whose organism died watches a newborn; a newborn with no
        // free world gets a new one, stepped to the reference's tick.
        let mut free: Vec<usize> = (0..watchers.len())
            .filter(|&at| watchers[at].watched().is_none_or(|id| !ids.contains(&id)))
            .collect();
        for &id in &ids {
            if watchers.iter().all(|world| world.watched() != Some(id)) {
                let at = free.pop().unwrap_or_else(|| {
                    let mut world = found(config);
                    for _ in 0..tick {
                        step(&mut world);
                    }
                    watchers.push(world);
                    watchers.len() - 1
                });
                watchers[at].watch(Some(id));
            }
        }
        let report = step(&mut reference);
        for world in &mut watchers {
            assert_eq!(step(world), report, "tick {tick}");
        }

        let mut records = Vec::with_capacity(rows.len());
        for row in &rows {
            let watcher = watchers
                .iter()
                .find(|world| world.watched() == Some(row.id))
                .expect("watched");
            let record = *watcher.watched_transactions().last().expect("a record");
            assert_eq!(record.tick, from);
            replay(&record, row, rho);
            match reference.population().get(row.id) {
                Some(next) => {
                    assert_eq!(next.energy.to_bits(), record.closing.to_bits());
                    if let Some(motion) = record.motion {
                        assert_eq!(bits64(&next.velocity), bits64(&motion.velocity));
                    }
                    assert_eq!(snapshot(watcher, row.id).transactions.last(), Some(&record));
                }
                None => {
                    assert_eq!(record.closing, 0.0);
                    assert!(report.deaths.iter().any(|death| death.id == row.id));
                    seen.deaths += 1;
                }
            }
            let born: Vec<OrganismId> = report
                .births
                .iter()
                .filter(|birth| birth.parent == row.id)
                .map(|birth| birth.child)
                .collect();
            let refused = report
                .rejected
                .iter()
                .find(|rejected| rejected.parent == row.id);
            match record.reproduction {
                Some(ReproductionOutcome::Born { child, split }) => {
                    assert_eq!(born, [child]);
                    let newborn = reference.population().get(child).expect("born");
                    assert_eq!(newborn.energy.to_bits(), split.child.to_bits());
                    seen.births += 1;
                }
                Some(ReproductionOutcome::Rejected { candidates, .. }) => {
                    assert!(born.is_empty());
                    assert_eq!(
                        refused.map(|rejected| rejected.candidates.len()),
                        Some(candidates)
                    );
                    seen.rejected += 1;
                }
                other => {
                    assert!(born.is_empty() && refused.is_none(), "{other:?}");
                    seen.cooling += usize::from(matches!(
                        other,
                        Some(ReproductionOutcome::CoolingDown { .. })
                    ));
                }
            }
            seen.emitted += usize::from(
                record
                    .emission
                    .is_some_and(|emission| emission.cost.paid > 0.0),
            );
            seen.scaled += usize::from(record.motion.is_some_and(|motion| motion.scaled));
            seen.moving += usize::from(
                record
                    .motion
                    .is_some_and(|motion| motion.velocity != [0.0, 0.0]),
            );
            seen.tiny += usize::from(record.tiny > 0.0);
            records.push(record);
        }
        assert_eq!(born_count(&records), report.births.len());

        let closure = &report.closure;
        let check = |name: &str, ledger: f64, records: f64| {
            assert_eq!(
                ledger.to_bits(),
                records.to_bits(),
                "{name} at tick {tick}: the ledger has {ledger}, the records {records}"
            );
        };
        check(
            "toxicity",
            closure.flow(Flow::Toxicity),
            total(records.iter().map(|record| record.uptake.toxicity.paid)),
        );
        let parts: [(Flow, Part); 3] = [
            (Flow::Maintenance, |maintenance| maintenance.paid.basal),
            (Flow::SensorCost, |maintenance| maintenance.paid.sensor),
            (Flow::ControllerCost, |maintenance| {
                maintenance.paid.controller
            }),
        ];
        for (flow, part) in parts {
            check(
                flow.name(),
                closure.flow(flow),
                total(
                    records
                        .iter()
                        .filter_map(|record| record.maintenance.as_ref().map(part))
                        .filter(|&paid| paid > 0.0),
                ),
            );
        }
        check(
            "motion",
            closure.flow(Flow::MotionCost),
            total(
                records
                    .iter()
                    .filter_map(|record| record.motion.map(|motion| motion.cost.paid)),
            ),
        );
        check(
            "tiny reservoir",
            closure.flow(Flow::TinyReservoir),
            total(
                records
                    .iter()
                    .filter(|record| !attempted(record))
                    .chain(records.iter().filter(|record| attempted(record)))
                    .map(|record| record.tiny)
                    .filter(|&cleared| cleared > 0.0),
            ),
        );
        check(
            "uptake debited",
            closure.debited(Transfer::Uptake),
            total(records.iter().map(|record| record.uptake.absorbed)),
        );
        check(
            "uptake credited",
            closure.credited(Transfer::Uptake),
            total(records.iter().map(|record| record.uptake.gain)),
        );
        let splits: Vec<_> = records
            .iter()
            .filter_map(|record| match record.reproduction {
                Some(ReproductionOutcome::Born { split, .. }) => Some(split),
                _ => None,
            })
            .collect();
        check(
            "child endowment debited",
            closure.debited(Transfer::ChildEndowment),
            total(splits.iter().map(|split| split.debited())),
        );
        check(
            "child endowment credited",
            closure.credited(Transfer::ChildEndowment),
            total(splits.iter().map(|split| split.child)),
        );
        check(
            "emission debited",
            closure.debited(Transfer::Emission),
            total(
                records
                    .iter()
                    .filter_map(|record| record.emission.map(|emission| emission.cost.paid))
                    .filter(|&paid| paid > 0.0),
            ),
        );
    }
    seen.worlds = watchers.len();
    eprintln!("{seen:?}");
    seen
}

fn born_count(records: &[Transactions]) -> usize {
    records
        .iter()
        .filter(|record| matches!(record.reproduction, Some(ReproductionOutcome::Born { .. })))
        .count()
}

/// The watch keeps the last `RECENT_STEPS` steps, oldest first and one per
/// step, the newest the step just published.
#[test]
fn the_watch_keeps_its_last_steps() {
    let config = small(1);
    let mut world = found(&config);
    let id = world.population().ids().next().expect("a founder");
    world.watch(Some(id));
    for steps in 1..=RECENT_STEPS + 3 {
        let from = world.tick();
        step(&mut world);
        let transactions = snapshot(&world, id).transactions;
        assert_eq!(transactions.len(), steps.min(RECENT_STEPS));
        assert_eq!(transactions.last().map(|record| record.tick), Some(from));
        for pair in transactions.windows(2) {
            assert_eq!(pair[0].tick.next().ok(), Some(pair[1].tick));
        }
    }
}
