//! FL-032's closing evidence (plan §23.1: whole-tick parity, affordability,
//! no partial commit after failure). Every stage FL-031 and FL-032 put on
//! Metal runs there at once, on one device owner: phase 2's field step,
//! phase 3's sampling, phase 5's uptake, phase 6's chemistry, maintenance,
//! motion and emission, phase 7's proposal, phase 8's mutation, phase 9's
//! reduction and phase 10's field check, with phase 4's controllers on
//! Metal too. The world is then the reference world at every tick: its
//! digest, the watched organism's transactions, each tick's births and
//! newborn genomes and its published field check, bit for bit. That holds
//! in fertile worlds whose births are mutated on the device and in worlds
//! whose organisms die at every budget stage (§8.2). A failure at any one
//! device stage, leaving damaged outputs behind wherever its port allows,
//! hands that stage's work to the reference and drops that backend alone
//! (§8.3): the tick stays the reference's, every other backend serves every
//! tick, and the shared owner is left with nothing leased, running or
//! retiring.

use flife_backend_metal::{
    ChemistryConstants, ChemistryOrganism, ChemistryOutcome, ChemistryPacket, DeviceOwner,
    EmissionOrganism, EmissionOutcome, MetalBackend, MetalField, MetalSensors, MetalSettle,
    MetalUptake, MotionConstants, MotionOrganism, MotionOutcome, MutationBirth, MutationConstants,
    MutationCounts, MutationValues, ProposalConstants, ProposalOrganism, ProposalOutcome,
};
use flife_core::{CoreError, TimeStep, ValidatedConfig};
use flife_engine::{Arrangement, BudgetStage, DeathCause, NumericalProfile, Stepped, World};
use flife_field::{
    ConeTolerance, DepositSummary, Deposits, ExchangeStencil, FieldBackend, FieldGrid, FieldPair,
    FieldScan, FieldState, FieldStepReport, FieldSummary, Grants, RawSample, RequestRule,
    SensorBackend, SensorPyramid, SensorQuery, SettleBackend, Source, SourceKernel, TransportPlan,
    UptakeBackend, UptakeCell,
};
use flife_genome::Genome;
use flife_neural::{ACTION_WIDTH, ControllerBackend, ControllerJob};
use std::num::NonZeroUsize;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// The preset with each line replaced, each found once.
fn with(lines: &[(&str, &str)]) -> String {
    lines.iter().fold(PRESET.to_owned(), |toml, (from, to)| {
        assert_eq!(toml.matches(from).count(), 1, "{from}");
        toml.replace(from, to)
    })
}

/// The preset whose founders propose from tick 2 (a cooldown of 0.01 s)
/// and can afford viable births (an attempt minimum of 1.0), so births
/// arrive from tick 3; half of them change member, and only weights
/// mutate, one in 10,000, in weight dtype `dtype`.
fn fertile(dtype: &str) -> String {
    let dtype = format!("canonical_weight_dtype = \"{dtype}\"");
    with(&[
        ("cooldown_seconds = 0.5", "cooldown_seconds = 0.01"),
        (
            "minimum_parent_energy_before_attempt = 3.0",
            "minimum_parent_energy_before_attempt = 1.0",
        ),
        (
            "probability_per_birth = 0.01",
            "probability_per_birth = 0.5",
        ),
        ("weight_probability = 0.002", "weight_probability = 0.0001"),
        (
            "chemistry_standard_deviation = 0.01",
            "chemistry_standard_deviation = 0.0",
        ),
        (
            "sensing_log_scale_standard_deviation = 0.02",
            "sensing_log_scale_standard_deviation = 0.0",
        ),
        ("canonical_weight_dtype = \"f32\"", dtype.as_str()),
    ])
}

/// The preset with another motion cost and maximum emission rate.
fn variant(motion: &str, rate: &str) -> String {
    with(&[
        (
            "motion_energy_per_speed_squared_second = 0.002",
            &format!("motion_energy_per_speed_squared_second = {motion}"),
        ),
        (
            "maximum_fractional_rate_per_second = 0.25",
            &format!("maximum_fractional_rate_per_second = {rate}"),
        ),
    ])
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure is a test failure"
)]
fn found_from(toml: &str) -> World {
    let config = ValidatedConfig::from_toml(toml).expect("the configuration validates");
    let mut world = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("the world founds");
    let workers = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    world
        .set_cpu_workers(NonZeroUsize::new(workers.min(16)).expect("nonzero"))
        .expect("workers");
    world
}

/// A device stage a backend can fail at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Phase 2's field step.
    Advance,
    /// Phase 3's raw sampling.
    Sample,
    /// Phase 4's controllers.
    Evaluate,
    /// Phase 5's densities and requests.
    Request,
    /// Phase 5's demands and quanta.
    Resolve,
    /// Phase 6's chemistry and maintenance.
    Chemistry,
    /// Phase 6's motion.
    Motion,
    /// Phase 6's emission.
    Emission,
    /// Phase 7's reproduction proposal.
    Proposal,
    /// Phase 8's parameter mutation.
    Mutation,
    /// Phase 9's grants' debit and deposit reductions.
    Reduce,
    /// Phase 10's field check.
    Check,
}

const STAGES: [Stage; 12] = [
    Stage::Advance,
    Stage::Sample,
    Stage::Evaluate,
    Stage::Request,
    Stage::Resolve,
    Stage::Chemistry,
    Stage::Motion,
    Stage::Emission,
    Stage::Proposal,
    Stage::Mutation,
    Stage::Reduce,
    Stage::Check,
];

/// Where a backend fails: at the `at`-th call of `stage`, or never.
#[derive(Debug, Clone, Copy)]
struct Plant {
    stage: Option<Stage>,
    at: u64,
}

const NEVER: Plant = Plant { stage: None, at: 0 };

/// A Metal backend that fails as planted, after doing the work and
/// damaging what it wrote wherever its port allows a failing backend to.
#[derive(Debug)]
struct Failing<B> {
    inner: B,
    plant: Plant,
    calls: u64,
}

impl<B> Failing<B> {
    const fn new(inner: B, plant: Plant) -> Self {
        Self {
            inner,
            plant,
            calls: 0,
        }
    }

    /// Whether this call of `stage` is the planted failure.
    fn fails(&mut self, stage: Stage) -> bool {
        if self.plant.stage != Some(stage) {
            return false;
        }
        self.calls += 1;
        self.calls == self.plant.at
    }
}

fn planted(stage: Stage) -> CoreError {
    CoreError::StateConflict {
        operation: "planted stage failure",
        detail: format!("the test's backend fails at {stage:?}"),
    }
}

/// Fail at `stage` with `outputs` scrambled (reversed), as a device failing
/// part way could leave them.
fn scrambled<T>(stage: Stage, outputs: &mut [T]) -> Result<(), CoreError> {
    outputs.reverse();
    Err(planted(stage))
}

impl FieldBackend for Failing<MetalField> {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn advance(
        &mut self,
        pair: &mut FieldPair,
        plan: &TransportPlan,
        kernel: &SourceKernel,
        sources: &[Source],
        step: TimeStep,
    ) -> Result<FieldStepReport, CoreError> {
        let report = self.inner.advance(pair, plan, kernel, sources, step)?;
        if self.fails(Stage::Advance) {
            pair.stage().1.energy_mut()[100] = -1.0;
            return Err(planted(Stage::Advance));
        }
        Ok(report)
    }

    fn reduce(
        &mut self,
        field: &mut FieldState,
        grants: &mut Grants,
        deposits: [&mut Deposits; 2],
    ) -> Result<[DepositSummary; 2], CoreError> {
        // The port allows a failing reduction to change nothing (§8.3), so
        // it fails before reducing.
        if self.fails(Stage::Reduce) {
            return Err(planted(Stage::Reduce));
        }
        self.inner.reduce(field, grants, deposits)
    }

    fn check(
        &mut self,
        field: &FieldState,
        tolerance: ConeTolerance,
    ) -> Result<FieldScan, CoreError> {
        let scan = self.inner.check(field, tolerance)?;
        if self.fails(Stage::Check) {
            return Err(planted(Stage::Check));
        }
        Ok(scan)
    }
}

impl SensorBackend for Failing<MetalSensors> {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn sample(
        &mut self,
        pyramid: &SensorPyramid,
        field: &FieldState,
        queries: &[SensorQuery],
        samples: &mut Vec<RawSample>,
    ) -> Result<(), CoreError> {
        self.inner.sample(pyramid, field, queries, samples)?;
        if self.fails(Stage::Sample) {
            return scrambled(Stage::Sample, samples);
        }
        Ok(())
    }
}

impl ControllerBackend for Failing<MetalBackend> {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn resident_bytes(&self) -> u64 {
        self.inner.resident_bytes()
    }

    fn forget(&mut self) {
        self.inner.forget();
    }

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
    ) -> Result<(), CoreError> {
        self.inner.evaluate(jobs, outputs)?;
        if self.fails(Stage::Evaluate) {
            return scrambled(Stage::Evaluate, outputs);
        }
        Ok(())
    }
}

impl UptakeBackend for Failing<MetalUptake> {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn request(
        &mut self,
        field: &FieldState,
        rule: &RequestRule,
        stencils: &[ExchangeStencil],
        densities: &mut Vec<f64>,
        requests: &mut Vec<f64>,
    ) -> Result<(), CoreError> {
        self.inner
            .request(field, rule, stencils, densities, requests)?;
        if self.fails(Stage::Request) {
            densities.reverse();
            return scrambled(Stage::Request, requests);
        }
        Ok(())
    }

    fn resolve(
        &mut self,
        cells: &[UptakeCell],
        requested: &[f64],
        demanded: &mut Vec<f64>,
        quanta: &mut Vec<u32>,
    ) -> Result<(), CoreError> {
        self.inner.resolve(cells, requested, demanded, quanta)?;
        if self.fails(Stage::Resolve) {
            demanded.reverse();
            return scrambled(Stage::Resolve, quanta);
        }
        Ok(())
    }
}

impl SettleBackend for Failing<MetalSettle> {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn chemistry(
        &mut self,
        observed: &FieldState,
        constants: &ChemistryConstants,
        organisms: &[ChemistryOrganism],
        packets: &[ChemistryPacket],
        out: &mut Vec<ChemistryOutcome>,
    ) -> Result<(), CoreError> {
        self.inner
            .chemistry(observed, constants, organisms, packets, out)?;
        if self.fails(Stage::Chemistry) {
            return scrambled(Stage::Chemistry, out);
        }
        Ok(())
    }

    fn motion(
        &mut self,
        constants: &MotionConstants,
        organisms: &[MotionOrganism],
        out: &mut Vec<MotionOutcome>,
    ) -> Result<(), CoreError> {
        self.inner.motion(constants, organisms, out)?;
        if self.fails(Stage::Motion) {
            return scrambled(Stage::Motion, out);
        }
        Ok(())
    }

    fn emission(
        &mut self,
        grid: &FieldGrid,
        efficiency: f64,
        organisms: &[EmissionOrganism],
        out: &mut Vec<EmissionOutcome>,
    ) -> Result<(), CoreError> {
        self.inner.emission(grid, efficiency, organisms, out)?;
        if self.fails(Stage::Emission) {
            return scrambled(Stage::Emission, out);
        }
        Ok(())
    }

    fn proposal(
        &mut self,
        constants: &ProposalConstants,
        organisms: &[ProposalOrganism],
        out: &mut Vec<ProposalOutcome>,
    ) -> Result<(), CoreError> {
        self.inner.proposal(constants, organisms, out)?;
        if self.fails(Stage::Proposal) {
            return scrambled(Stage::Proposal, out);
        }
        Ok(())
    }

    fn mutation(
        &mut self,
        constants: &MutationConstants,
        births: &[MutationBirth],
        mut values: MutationValues<'_>,
        out: &mut Vec<MutationCounts>,
    ) -> Result<(), CoreError> {
        let reborrowed = match &mut values {
            MutationValues::F32(values) => MutationValues::F32(values),
            MutationValues::F16(values) => MutationValues::F16(values),
        };
        self.inner.mutation(constants, births, reborrowed, out)?;
        if self.fails(Stage::Mutation) {
            match values {
                MutationValues::F32(values) => values.reverse(),
                MutationValues::F16(values) => values.reverse(),
            }
            return scrambled(Stage::Mutation, out);
        }
        Ok(())
    }
}

/// `None` off macOS, where `open` must refuse with its typed error.
#[expect(clippy::panic, reason = "test helper")]
fn settle() -> Option<MetalSettle> {
    match MetalSettle::open() {
        Ok(settle) => Some(settle),
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => None,
        Err(error) => panic!("this host should have Metal: {error}"),
    }
}

/// Every Metal backend in `world`, failing as planted: the field, sensors,
/// uptake and settlement on one device owner (returned), and the
/// controllers.
#[expect(clippy::expect_used, reason = "test helper")]
fn install(world: &mut World, plant: Plant) -> Option<DeviceOwner> {
    let settle = settle()?;
    let owner = settle.owner().clone();
    world.set_field_backend(Some(Box::new(Failing::new(
        MetalField::with_owner(owner.clone()).expect("the field kernels compile"),
        plant,
    ))));
    world.set_sensor_backend(Some(Box::new(Failing::new(
        MetalSensors::with_owner(owner.clone()).expect("the sensor kernels compile"),
        plant,
    ))));
    world.set_uptake_backend(Some(Box::new(Failing::new(
        MetalUptake::with_owner(owner.clone()).expect("the uptake kernels compile"),
        plant,
    ))));
    world.set_controller_backend(Some(Box::new(Failing::new(
        MetalBackend::open().expect("a Metal device has the controllers too"),
        plant,
    ))));
    world.set_settle_backend(Some(Box::new(Failing::new(settle, plant))));
    Some(owner)
}

/// What a race saw: the first tick whose digests, watched transactions,
/// births, newborn genomes or published field checks differ; the first
/// tick at which a backend failed; the births; and the deaths by stage
/// (chemistry, maintenance, motion, emission).
#[derive(Debug, Default)]
struct Raced {
    difference: Option<u64>,
    failed: Option<u64>,
    births: usize,
    deaths: [usize; 4],
}

/// Whether two genomes are the same, bit for bit.
fn same_genome(a: &Genome, b: &Genome) -> bool {
    let bits = |values: &[f64]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    let (x, y) = (a.traits(), b.traits());
    a.topology() == b.topology()
        && a.parameters().same_bits(&b.parameters())
        && bits(x.affinity()) == bits(y.affinity())
        && bits(x.motion_response()) == bits(y.motion_response())
        && x.sensing_scale().to_bits() == y.sensing_scale().to_bits()
}

/// A field check's summary, its sums as bits.
fn summary_bits(summary: &FieldSummary) -> (u64, u64, Option<(u32, u32)>, u64) {
    (
        summary.energy_total.to_bits(),
        summary.max_cone_residual.to_bits(),
        summary.max_cone_residual_cell,
        summary.empty_cells,
    )
}

/// Whether any of `world`'s backends has failed.
fn any_failure(world: &World) -> bool {
    world.field_report().failure.is_some()
        || world.sensor_report().failure.is_some()
        || world.uptake_report().failure.is_some()
        || world.controller_report().failure.is_some()
        || world.settle_report().failure.is_some()
}

/// Step both worlds, each watching the same organism.
#[expect(clippy::expect_used, clippy::panic, reason = "test helper")]
fn race(reference: &mut World, other: &mut World, ticks: u64) -> Raced {
    let watched = reference.population().ids().next();
    reference.watch(watched);
    other.watch(watched);
    let mut raced = Raced::default();
    for step in 1..=ticks {
        let expected = reference.step().expect("the reference steps");
        let got = other.step().expect("the other world steps");
        let (Stepped::Published(expected), Stepped::Published(report)) = (expected, got) else {
            panic!("a tick did not publish");
        };
        let births_same = expected.births == report.births
            && report.births.iter().all(|birth| {
                match (
                    reference.population().genome(birth.child),
                    other.population().genome(birth.child),
                ) {
                    (Some(a), Some(b)) => same_genome(a, b),
                    _ => false,
                }
            });
        raced.births += report.births.len();
        for death in &report.deaths {
            let stage = match death.cause {
                DeathCause::Exhausted(BudgetStage::Chemistry) => 0,
                DeathCause::Exhausted(BudgetStage::Maintenance) => 1,
                DeathCause::Exhausted(BudgetStage::Motion) => 2,
                DeathCause::Exhausted(BudgetStage::Emission) => 3,
                _ => continue,
            };
            raced.deaths[stage] += 1;
        }
        let same = births_same
            && summary_bits(&expected.field) == summary_bits(&report.field)
            && reference.digest() == other.digest()
            && reference
                .watched_transactions()
                .eq(other.watched_transactions());
        if raced.difference.is_none() && !same {
            raced.difference = Some(step);
        }
        if raced.failed.is_none() && any_failure(other) {
            raced.failed = Some(step);
        }
    }
    raced
}

/// Each backend's service, as (device, reference) counts: the field's
/// steps, reductions and checks, the sensors', uptake's and settlement's
/// ticks, and the controllers' reference ticks; then which backends
/// recorded a failure.
#[derive(Debug, PartialEq, Eq)]
struct Service {
    steps: (u64, u64),
    reductions: (u64, u64),
    checks: (u64, u64),
    sensors: (u64, u64),
    uptake: (u64, u64),
    settle: (u64, u64),
    controllers: u64,
    failed: Vec<&'static str>,
}

fn service(world: &World) -> Service {
    let field = world.field_report();
    let sensors = world.sensor_report();
    let uptake = world.uptake_report();
    let controllers = world.controller_report();
    let settle = world.settle_report();
    let failed = [
        ("field", field.failure.as_deref()),
        ("sensors", sensors.failure.as_deref()),
        ("uptake", uptake.failure.as_deref()),
        ("controllers", controllers.failure.as_deref()),
        ("settle", settle.failure.as_deref()),
    ]
    .into_iter()
    .filter_map(|(name, failure)| {
        failure.map(|failure| {
            assert!(failure.contains("planted stage failure"), "{failure}");
            name
        })
    })
    .collect();
    Service {
        steps: (field.steps, field.reference_steps),
        reductions: (field.reductions, field.reference_reductions),
        checks: (field.checks, field.reference_checks),
        sensors: (sensors.ticks, sensors.reference_ticks),
        uptake: (uptake.ticks, uptake.reference_ticks),
        settle: (settle.ticks, settle.reference_ticks),
        controllers: controllers.reference_ticks,
        failed,
    }
}

/// Every backend served every one of `ticks` ticks, none failing.
const fn full(ticks: u64) -> Service {
    Service {
        steps: (ticks, 0),
        reductions: (ticks, 0),
        checks: (ticks, 0),
        sensors: (ticks, 0),
        uptake: (ticks, 0),
        settle: (ticks, 0),
        controllers: 0,
        failed: Vec::new(),
    }
}

/// Nothing leased, bound or retiring on the shared owner.
fn assert_idle(owner: &DeviceOwner) {
    let usage = owner.usage();
    assert_eq!(
        (
            usage.retiring,
            usage.read_leases,
            usage.write_leases,
            usage.running
        ),
        (0, [0; 3], 0, 0),
        "{usage:?}"
    );
}

#[test]
fn fertile_worlds_with_every_stage_on_metal_are_the_reference_worlds() {
    for dtype in ["f32", "f16"] {
        let toml = fertile(dtype);
        let mut reference = found_from(&toml);
        let mut metal = found_from(&toml);
        let Some(owner) = install(&mut metal, NEVER) else {
            return;
        };
        let raced = race(&mut reference, &mut metal, 12);
        let service = service(&metal);
        println!(
            "{dtype}: {raced:?}: {service:?}: {:?}",
            metal.settle_report()
        );
        assert_eq!(raced.difference, None, "{dtype}: the worlds diverged");
        assert_eq!(service, full(12), "{dtype}");
        assert!(raced.births > 0, "{dtype}: {raced:?}");
        assert!(
            metal.settle_report().mutated >= raced.births as u64,
            "{dtype}: every first candidate mutated on the device"
        );
        assert!(metal.controller_report().batched > 0);
        assert_idle(&owner);
    }
}

/// The two variants of `settle_world`'s affordability test, with every
/// stage on Metal: motion a thousand times as costly with emission draining
/// budgets four thousand times as fast, and emission so fast that a spend
/// fraction rounds to one. Between them organisms die at every stage.
#[test]
fn worlds_whose_organisms_die_at_every_stage_with_every_stage_on_metal_are_the_reference_worlds() {
    let mut deaths = [0; 4];
    for (motion, rate) in [("2.0", "1000.0"), ("0.002", "1.0e7")] {
        let changed = format!("motion cost {motion}, emission rate {rate}");
        let toml = variant(motion, rate);
        let mut reference = found_from(&toml);
        let mut metal = found_from(&toml);
        let Some(owner) = install(&mut metal, NEVER) else {
            return;
        };
        let raced = race(&mut reference, &mut metal, 60);
        let service = service(&metal);
        println!("{changed}: {raced:?}: {service:?}");
        assert_eq!(raced.difference, None, "{changed}: the worlds diverged");
        assert_eq!(service, full(60), "{changed}");
        assert_idle(&owner);
        for (total, count) in deaths.iter_mut().zip(raced.deaths) {
            *total += count;
        }
    }
    assert!(deaths.iter().all(|&count| count > 0), "{deaths:?}");
}

/// What the run with `stage` failing must show, given the tick it failed
/// at (`at`) of `ticks`: that backend served the ticks before it, the
/// reference did that tick's part, and nothing more ran on it; every other
/// backend served every tick.
fn expected(stage: Stage, at: u64, ticks: u64) -> Service {
    let before = at - 1;
    let mut service = full(ticks);
    match stage {
        Stage::Advance => {
            service.steps = (before, 1);
            service.reductions = (before, 0);
            service.checks = (before, 0);
            service.failed = vec!["field"];
        }
        Stage::Reduce => {
            service.steps = (at, 0);
            service.reductions = (before, 1);
            service.checks = (before, 0);
            service.failed = vec!["field"];
        }
        Stage::Check => {
            service.steps = (at, 0);
            service.reductions = (at, 0);
            service.checks = (before, 1);
            service.failed = vec!["field"];
        }
        Stage::Sample => {
            service.sensors = (before, 1);
            service.failed = vec!["sensors"];
        }
        Stage::Evaluate => {
            service.controllers = 1;
            service.failed = vec!["controllers"];
        }
        Stage::Request | Stage::Resolve => {
            service.uptake = (before, 1);
            service.failed = vec!["uptake"];
        }
        Stage::Chemistry | Stage::Motion | Stage::Emission | Stage::Proposal => {
            service.settle = (before, 1);
            service.failed = vec!["settle"];
        }
        // That tick's phases 6 and 7 ran on the device, and the reference
        // proposed its births.
        Stage::Mutation => {
            service.settle = (at, 1);
            service.failed = vec!["settle"];
        }
    }
    service
}

#[test]
fn a_failure_at_any_device_stage_leaves_the_world_the_reference_world() {
    const TICKS: u64 = 10;
    for stage in STAGES {
        // The third call of each stage; mutation's second, since births
        // begin at the third tick.
        let at = if stage == Stage::Mutation { 2 } else { 3 };
        let toml = fertile("f32");
        let mut reference = found_from(&toml);
        let mut metal = found_from(&toml);
        let plant = Plant {
            stage: Some(stage),
            at,
        };
        let Some(owner) = install(&mut metal, plant) else {
            return;
        };
        let raced = race(&mut reference, &mut metal, TICKS);
        let service = service(&metal);
        println!("{stage:?}: {raced:?}: {service:?}");
        assert_eq!(raced.difference, None, "{stage:?}: the worlds diverged");
        let failed = raced
            .failed
            .unwrap_or_else(|| panic!("{stage:?}: no backend failed"));
        assert!(failed < TICKS, "{stage:?}: failed at {failed}");
        assert_eq!(service, expected(stage, failed, TICKS), "{stage:?}");
        assert!(raced.births > 0, "{stage:?}: {raced:?}");
        assert_idle(&owner);
    }
}
