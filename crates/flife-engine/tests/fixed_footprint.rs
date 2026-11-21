//! FL-053: end-to-end `World` verification and grid-refinement study for
//! `fixed-world-gaussian-v1` (plan §4.3, §11.6, §18.5, INV-01..INV-05,
//! FL-043 memory admission, checkpoint round-trip, and honest CIC-only
//! backend refusal).

use std::num::NonZeroUsize;

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::{CoreError, RunConfig, ValidatedConfig, validate};
use flife_engine::{
    Advance, Arrangement, Disposition, Intervention, NumericalProfile, Stepped, TickReport,
    WORLD_CHECKPOINT_CHUNKS, World,
};
use flife_field::settle::{
    ChemistryConstants, ChemistryOrganism, ChemistryOutcome, ChemistryPacket, EmissionOrganism,
    EmissionOutcome, MotionConstants, MotionOrganism, MotionOutcome, MutationBirth,
    MutationConstants, MutationCounts, MutationValues, ProposalConstants, ProposalOrganism,
    ProposalOutcome, SettleBackend,
};
use flife_field::{
    ExchangeKernel, ExchangeStencil, FIXED_WORLD_GAUSSIAN_V1, FieldGrid, FieldState, RequestRule,
    UptakeBackend, UptakeCell,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

fn appendix_a_toml() -> String {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    appendix[start..start + length].to_owned()
}

fn config_at(grid: u32, exchange_kernel: &str, radius: Option<f64>) -> RunConfig {
    let mut cfg = RunConfig::from_toml(&appendix_a_toml()).expect("parse Appendix A");
    cfg.world.width = 32.0;
    cfg.world.height = 32.0;
    cfg.world.grid_width = grid;
    cfg.world.grid_height = grid;
    cfg.world.exchange_kernel = exchange_kernel.to_owned();
    cfg.world.exchange_radius_world_units = radius;
    // Keep physical initial field energy density constant across resolutions:
    // total field energy = 1024.0 regardless of grid cell count.
    cfg.field_initialization.energy_per_cell = 1024.0 / f64::from(grid * grid);
    // Keep physical source width, count, and power fixed across resolutions.
    cfg.sources.count = 3;
    cfg.sources.sigma_world_units = 2.0;
    cfg.sources.power_per_source = 20.0;
    // Keep physical sensing scales well-defined across 32..128 grids.
    cfg.sensors.minimum_scale_world_units = 1.0;
    cfg.sensors.initial_scale_world_units = 2.0;
    cfg.sensors.maximum_scale_world_units = 4.0;
    // Compact population for fast multi-resolution refinement tests.
    cfg.population.initial_count = 48;
    cfg.population.maximum_count = 128;
    cfg.execution.cpu_workers = 1;
    cfg
}

fn step_world(world: &mut World) -> TickReport {
    match world.step().expect("step") {
        Stepped::Published(report) => *report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

fn step_with_interventions(world: &mut World, interventions: &[Intervention]) -> TickReport {
    match world.advance_with(interventions).expect("advance_with") {
        Advance::Ready(staged) => staged.publish().expect("publish"),
        Advance::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

fn field_bits(world: &World) -> Vec<u32> {
    world
        .field()
        .as_slice()
        .iter()
        .map(|v| v.to_bits())
        .collect()
}

#[test]
fn fixed_world_exchange_config_validation_and_canonical_hash() {
    // Default `grid-cic-v1` canonical TOML omits `exchange_radius_world_units`.
    let cic = validate(config_at(32, "grid-cic-v1", None)).expect("valid cic");
    assert!(!cic.canonical_toml().contains("exchange_radius_world_units"));

    // `grid-cic-v1` refuses a radius.
    let err = validate(config_at(32, "grid-cic-v1", Some(1.5))).expect_err("cic with radius");
    assert!(matches!(err, CoreError::InvalidConfig(_)), "{err:?}");

    // `fixed-world-gaussian-v1` requires a radius.
    let err = validate(config_at(32, FIXED_WORLD_GAUSSIAN_V1, None)).expect_err("missing radius");
    assert!(matches!(err, CoreError::InvalidConfig(_)), "{err:?}");

    // Radius smaller than cell spacing is refused.
    let err = validate(config_at(32, FIXED_WORLD_GAUSSIAN_V1, Some(0.5))).expect_err("radius < h");
    assert!(matches!(err, CoreError::InvalidConfig(_)), "{err:?}");

    // Radius >= half world side is refused (torus self-overlap).
    let err =
        validate(config_at(32, FIXED_WORLD_GAUSSIAN_V1, Some(16.0))).expect_err("radius >= L/2");
    assert!(matches!(err, CoreError::InvalidConfig(_)), "{err:?}");

    // Footprint exceeding 4,096 cells is refused.
    let err = validate(config_at(256, FIXED_WORLD_GAUSSIAN_V1, Some(5.0)))
        .expect_err("footprint > 4096 cells");
    assert!(matches!(err, CoreError::InvalidConfig(_)), "{err:?}");

    // Valid fixed-world configuration round-trips with identical canonical hash.
    let valid = validate(config_at(64, FIXED_WORLD_GAUSSIAN_V1, Some(1.5))).expect("valid");
    assert!(
        valid
            .canonical_toml()
            .contains("exchange_radius_world_units = 1.5")
    );
    let reparsed = ValidatedConfig::from_toml(&valid.canonical_toml()).expect("reparse");
    assert_eq!(valid.config_hash(), reparsed.config_hash());
}

#[test]
fn fixed_world_gaussian_world_refinement_study_and_conservation() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let mut tick1_uptake = Vec::new();
    let mut tick1_requested = Vec::new();
    let mut tick10_org_energy = Vec::new();
    let mut tick10_field_energy = Vec::new();

    for &grid in &[32_u32, 64, 128] {
        let validated = validate(config_at(grid, FIXED_WORLD_GAUSSIAN_V1, Some(1.5)))
            .expect("validate refinement config");
        let mut world =
            World::found(&validated, Arrangement::Compact, profile).expect("found world");
        assert_eq!(
            world.exchange_kernel(),
            ExchangeKernel::fixed_world_gaussian(1.5).expect("kernel")
        );

        // FL-043: peak memory admission accounts for expanded fixed-world scratch,
        // and stepping does not grow footprint.
        let admission = world.admit_memory(false, 0, 0).expect("admit memory");
        assert!(admission.peak_bytes <= admission.budget_bytes);
        let footprint_before = world.footprint();

        // Snapshot inspection uses the fixed-world exchange footprint (>4 cells on 64²/128²).
        let first_id = world.population().iter().next().expect("founder").id;
        let snap = world
            .organism_snapshot(first_id)
            .expect("snapshot")
            .expect("present");
        if grid >= 64 {
            assert!(
                snap.local.cells.len() > 4,
                "expected >4 footprint cells on {grid}x{grid}, got {}",
                snap.local.cells.len()
            );
        }

        // Step 15 ticks, including a recycled removal intervention at tick 10 so
        // recycling deposits under `fixed-world-gaussian-v1` are also exercised.
        for step in 0..15_u32 {
            let report = if step == 10 {
                let target = world
                    .population()
                    .iter()
                    .next()
                    .map_or([16.0, 16.0], |o| o.position);
                step_with_interventions(
                    &mut world,
                    &[Intervention::RemoveOrganisms {
                        center: target,
                        radius: 1.5,
                        disposition: Disposition::Recycle,
                    }],
                )
            } else {
                step_world(&mut world)
            };
            if step == 0 {
                tick1_uptake.push(report.uptake.granted);
                tick1_requested.push(report.uptake.requested);
            }
            if step == 9 {
                tick10_org_energy.push(report.closure.closing().organisms());
                tick10_field_energy.push(report.closure.closing().field());
            }
        }

        let footprint_after = world.footprint();
        assert_eq!(
            footprint_after.field, footprint_before.field,
            "FL-043: field footprint must stay fixed on {grid}x{grid}"
        );
        assert_eq!(
            footprint_after.scratch, footprint_before.scratch,
            "FL-043: scratch footprint must stay fixed on {grid}x{grid}"
        );
        assert_eq!(
            footprint_after.staging, footprint_before.staging,
            "FL-043: staging footprint must stay fixed on {grid}x{grid}"
        );
        assert!(
            footprint_after.state <= footprint_before.state,
            "FL-043: state footprint must not grow on {grid}x{grid}: {footprint_after:?} vs \
             {footprint_before:?}"
        );
    }

    // Refinement convergence checks across 32² -> 64² -> 128² at fixed physical world:
    let req_diff_32_64 = (tick1_requested[1] - tick1_requested[0]).abs();
    let req_diff_64_128 = (tick1_requested[2] - tick1_requested[1]).abs();
    assert!(
        req_diff_64_128 < req_diff_32_64,
        "tick-1 uptake request should converge under refinement: 32..64={req_diff_32_64:e}, \
         64..128={req_diff_64_128:e}, values={tick1_requested:?}"
    );
    let upt_diff_32_64 = (tick1_uptake[1] - tick1_uptake[0]).abs();
    let upt_diff_64_128 = (tick1_uptake[2] - tick1_uptake[1]).abs();
    assert!(
        upt_diff_64_128 <= upt_diff_32_64 + 1e-6,
        "tick-1 uptake grant should converge under refinement: 32..64={upt_diff_32_64:e}, \
         64..128={upt_diff_64_128:e}, values={tick1_uptake:?}"
    );
    // Relative agreement between 64² and 128² on macroscopic energy at tick 10.
    let rel_field =
        (tick10_field_energy[2] - tick10_field_energy[1]).abs() / tick10_field_energy[2].max(1.0);
    let rel_org =
        (tick10_org_energy[2] - tick10_org_energy[1]).abs() / tick10_org_energy[2].max(1.0);
    assert!(
        rel_field < 0.02 && rel_org < 0.05,
        "64² vs 128² macroscopic energy drifted too far: field={tick10_field_energy:?}, \
         org={tick10_org_energy:?}"
    );
}

#[test]
fn fixed_world_gaussian_checkpoint_roundtrip_and_worker_invariance() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let validated = validate(config_at(64, FIXED_WORLD_GAUSSIAN_V1, Some(1.5))).expect("valid");

    // 1-worker continuous world vs 1-worker checkpointed + restored world.
    let mut continuous = World::found(&validated, Arrangement::Compact, profile).expect("found");
    let mut multi_worker =
        World::found(&validated, Arrangement::Compact, profile).expect("found 2");
    multi_worker
        .set_cpu_workers(NonZeroUsize::new(4).expect("4"))
        .expect("4 workers");

    for _ in 0..8 {
        step_world(&mut continuous);
        step_world(&mut multi_worker);
    }

    let mut writer = CheckpointWriter::new(&validated.config().model);
    continuous.write_checkpoint(&mut writer).expect("write");
    let bytes = writer.finish().expect("finish");
    let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("parse");
    let mut restored = World::read_checkpoint(&reader, profile).expect("read");

    for _ in 0..8 {
        step_world(&mut continuous);
        step_world(&mut restored);
        step_world(&mut multi_worker);
    }

    assert_eq!(field_bits(&continuous), field_bits(&restored));
    assert_eq!(field_bits(&continuous), field_bits(&multi_worker));
    assert_eq!(
        continuous.population().iter().collect::<Vec<_>>(),
        restored.population().iter().collect::<Vec<_>>()
    );
    assert_eq!(
        continuous.population().iter().collect::<Vec<_>>(),
        multi_worker.population().iter().collect::<Vec<_>>()
    );
    assert_eq!(continuous.ledger().state(), restored.ledger().state());
    assert_eq!(continuous.ledger().state(), multi_worker.ledger().state());
}

#[derive(Debug)]
struct StubUptakeBackend;

impl UptakeBackend for StubUptakeBackend {
    fn describe(&self) -> String {
        "stub-cic-uptake".to_owned()
    }

    fn request(
        &mut self,
        _field: &FieldState,
        _rule: &RequestRule,
        _stencils: &[ExchangeStencil],
        _densities: &mut Vec<f64>,
        _requests: &mut Vec<f64>,
    ) -> Result<(), CoreError> {
        panic!("CIC-only uptake backend must never be dispatched under fixed-world-gaussian-v1");
    }

    fn resolve(
        &mut self,
        _cells: &[UptakeCell],
        _requested: &[f64],
        _demanded: &mut Vec<f64>,
        _quanta: &mut Vec<u32>,
    ) -> Result<(), CoreError> {
        panic!("CIC-only uptake backend must never be dispatched under fixed-world-gaussian-v1");
    }
}

#[derive(Debug)]
struct StubSettleBackend;

impl SettleBackend for StubSettleBackend {
    fn describe(&self) -> String {
        "stub-cic-settle".to_owned()
    }

    fn chemistry(
        &mut self,
        _field: &FieldState,
        _constants: &ChemistryConstants,
        _organisms: &[ChemistryOrganism],
        _packets: &[ChemistryPacket],
        _out: &mut Vec<ChemistryOutcome>,
    ) -> Result<(), CoreError> {
        panic!("CIC-only settle backend must never be dispatched under fixed-world-gaussian-v1");
    }

    fn motion(
        &mut self,
        _constants: &MotionConstants,
        _organisms: &[MotionOrganism],
        _out: &mut Vec<MotionOutcome>,
    ) -> Result<(), CoreError> {
        panic!("CIC-only settle backend must never be dispatched under fixed-world-gaussian-v1");
    }

    fn emission(
        &mut self,
        _grid: &FieldGrid,
        _efficiency: f64,
        _organisms: &[EmissionOrganism],
        _out: &mut Vec<EmissionOutcome>,
    ) -> Result<(), CoreError> {
        panic!("CIC-only settle backend must never be dispatched under fixed-world-gaussian-v1");
    }

    fn proposal(
        &mut self,
        _constants: &ProposalConstants,
        _organisms: &[ProposalOrganism],
        _out: &mut Vec<ProposalOutcome>,
    ) -> Result<(), CoreError> {
        panic!("CIC-only settle backend must never be dispatched under fixed-world-gaussian-v1");
    }

    fn mutation(
        &mut self,
        _constants: &MutationConstants,
        _births: &[MutationBirth],
        _values: MutationValues<'_>,
        _counts: &mut Vec<MutationCounts>,
    ) -> Result<(), CoreError> {
        panic!("CIC-only settle backend must never be dispatched under fixed-world-gaussian-v1");
    }
}

#[test]
fn cic_only_backends_refuse_fixed_world_gaussian_honestly() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let validated = validate(config_at(32, FIXED_WORLD_GAUSSIAN_V1, Some(1.5))).expect("valid");
    let mut reference = World::found(&validated, Arrangement::Compact, profile).expect("ref");
    let mut with_backends =
        World::found(&validated, Arrangement::Compact, profile).expect("with backends");
    with_backends.set_uptake_backend(Some(Box::new(StubUptakeBackend)));
    with_backends.set_settle_backend(Some(Box::new(StubSettleBackend)));

    step_world(&mut reference);
    step_world(&mut with_backends);

    let urep = with_backends.uptake_report();
    assert_eq!(urep.ticks, 0);
    assert_eq!(urep.reference_ticks, 1);
    assert!(
        urep.failure
            .as_deref()
            .is_some_and(|msg| msg.contains(FIXED_WORLD_GAUSSIAN_V1)),
        "expected honest refusal naming kernel, got {:?}",
        urep.failure
    );

    let srep = with_backends.settle_report();
    assert_eq!(srep.ticks, 0);
    assert_eq!(srep.reference_ticks, 1);
    assert!(
        srep.failure
            .as_deref()
            .is_some_and(|msg| msg.contains(FIXED_WORLD_GAUSSIAN_V1)),
        "expected honest refusal naming kernel, got {:?}",
        srep.failure
    );

    assert_eq!(field_bits(&reference), field_bits(&with_backends));
    assert_eq!(
        reference.population().iter().collect::<Vec<_>>(),
        with_backends.population().iter().collect::<Vec<_>>()
    );
}
