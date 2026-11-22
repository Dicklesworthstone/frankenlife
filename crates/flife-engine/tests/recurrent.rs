//! FL-054: end-to-end `World` verification for `independent-recurrent-mlp-v1`
//! and `recurrent-104-36-catalog-v1` (plan §6.2–§6.6, §17.5, §23.1, §25.2):
//! schema validation, topology transition preservation, stateful replay across
//! checkpoints/workers/arrangements, parameter-count energy costing, bounded
//! memory admission, and honest refusal on stateless `ControllerBackend` ports.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::num::NonZeroUsize;

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::{CoreError, RunConfig, ValidatedConfig, validate};
use flife_engine::{
    Arrangement, NumericalProfile, Stepped, TickReport, WORLD_CHECKPOINT_CHUNKS, World,
};
use flife_genome::{
    DENSE_104_36_CATALOG_V1, RECURRENT_104_36_CATALOG_V1, WeightAddress, map_parameters,
};
use flife_neural::{
    ACTION_WIDTH, ControllerBackend, ControllerJob, ControllerScratch, DenseTopology,
    INDEPENDENT_DENSE_MLP_V1, INDEPENDENT_RECURRENT_MLP_V1, ReferenceController, SENSOR_WIDTH,
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

fn base_config(recurrent_width: Option<u32>) -> RunConfig {
    let mut cfg = RunConfig::from_toml(&appendix_a_toml()).expect("parse Appendix A");
    cfg.world.width = 32.0;
    cfg.world.height = 32.0;
    cfg.world.grid_width = 32;
    cfg.world.grid_height = 32;
    cfg.population.initial_count = 48;
    cfg.population.maximum_count = 256;
    cfg.population.initial_energy = 3.0;
    cfg.sources.count = 4;
    cfg.sources.power_per_source = 120.0;
    cfg.sources.sigma_world_units = 3.0;
    cfg.sources.path_direction_interval_seconds = 2.0;
    cfg.chemistry.uptake_capacity_per_second = 8.0;
    cfg.costs.basal_energy_per_second = 0.6;
    cfg.reproduction.minimum_parent_energy_before_attempt = 2.2;
    cfg.reproduction.maximum_attempt_rate_per_second = 4.0;
    cfg.reproduction.cooldown_seconds = 0.25;
    cfg.controller.hidden_widths = vec![8];
    cfg.controller.evaluation_interval_ticks = 2;
    cfg.architecture_mutation.enabled = true;
    cfg.architecture_mutation.probability_per_birth = 0.25;
    cfg.execution.cpu_workers = 1;
    if let Some(width) = recurrent_width {
        cfg.controller.family = INDEPENDENT_RECURRENT_MLP_V1.to_owned();
        cfg.controller.recurrent_state_width = Some(width);
        cfg.architecture_mutation.catalog = RECURRENT_104_36_CATALOG_V1.to_owned();
    } else {
        cfg.controller.family = INDEPENDENT_DENSE_MLP_V1.to_owned();
        cfg.controller.recurrent_state_width = None;
        cfg.architecture_mutation.catalog = DENSE_104_36_CATALOG_V1.to_owned();
    }
    cfg
}

fn step_world(world: &mut World) -> TickReport {
    match world.step().expect("step") {
        Stepped::Published(report) => *report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

#[test]
fn recurrent_world_config_validation_and_canonical_hash() {
    // Baseline dense config omits `recurrent_state_width`.
    let dense = validate(base_config(None)).expect("valid dense");
    assert!(!dense.canonical_toml().contains("recurrent_state_width"));
    assert_eq!(
        dense.report().default_parameter_count,
        (104 * 8 + 8) + (8 * 36 + 36)
    );

    // Dense config refuses `recurrent_state_width` and recurrent catalog.
    let mut bad_dense = base_config(None);
    bad_dense.controller.recurrent_state_width = Some(4);
    assert!(matches!(
        validate(bad_dense),
        Err(CoreError::InvalidConfig(_))
    ));

    let mut bad_catalog = base_config(None);
    bad_catalog.architecture_mutation.catalog = RECURRENT_104_36_CATALOG_V1.to_owned();
    assert!(matches!(
        validate(bad_catalog),
        Err(CoreError::InvalidConfig(_))
    ));

    // Recurrent config requires state width in 1..=16 and recurrent catalog.
    let mut missing_width = base_config(Some(8));
    missing_width.controller.recurrent_state_width = None;
    assert!(matches!(
        validate(missing_width),
        Err(CoreError::InvalidConfig(_))
    ));

    let mut too_wide = base_config(Some(17));
    too_wide.controller.recurrent_state_width = Some(17);
    assert!(matches!(
        validate(too_wide),
        Err(CoreError::InvalidConfig(_))
    ));

    let mut wrong_catalog = base_config(Some(8));
    wrong_catalog.architecture_mutation.catalog = DENSE_104_36_CATALOG_V1.to_owned();
    assert!(matches!(
        validate(wrong_catalog),
        Err(CoreError::InvalidConfig(_))
    ));

    // Valid recurrent config calculates exact recurrent parameter count and round-trips hash.
    let recurrent = validate(base_config(Some(8))).expect("valid recurrent");
    assert!(
        recurrent
            .canonical_toml()
            .contains("recurrent_state_width = 8")
    );
    assert_eq!(
        recurrent.report().default_parameter_count,
        (112 * 8 + 8) + (8 * 52 + 52)
    );
    let reparsed = ValidatedConfig::from_toml(&recurrent.canonical_toml()).expect("reparse");
    assert_eq!(recurrent.config_hash(), reparsed.config_hash());
}

#[test]
fn recurrent_topology_widening_preserves_actions_and_state_transition() {
    let small = DenseTopology::new_recurrent(&[8], 6).expect("small");
    let wide = DenseTopology::new_recurrent(&[32], 6).expect("wide");
    let mut parent = vec![0.0_f32; small.parameter_count()];
    for (i, slot) in parent.iter_mut().enumerate() {
        *slot = (((i as f64 * 0.73).sin()) * 0.35) as f32;
    }
    let mut child = vec![0.0_f32; wide.parameter_count()];
    map_parameters(&small, &parent, &wide, &mut child, |addr: WeightAddress| {
        ((addr.layer as u64) << 40) ^ ((addr.output as u64) << 20) ^ (addr.input as u64) ^ 0x9e37
    })
    .expect("widen recurrent");

    let parent_ctrl = ReferenceController::new(&small, &parent, 10.0).expect("parent ctrl");
    let child_ctrl = ReferenceController::new(&wide, &child, 10.0).expect("child ctrl");
    let mut scratch = ControllerScratch::new(&wide).expect("scratch");
    let mut inputs = [0.0_f64; SENSOR_WIDTH];
    for (i, slot) in inputs.iter_mut().enumerate() {
        *slot = (i as f64 * 0.19).cos() * 0.7;
    }
    let state = [0.25_f32, -0.5, 0.75, -0.1, 0.0, 0.9];
    let mut p_next = [0.0_f32; 6];
    let mut c_next = [0.0_f32; 6];
    let p_out = parent_ctrl
        .evaluate_recurrent(&inputs, &state, &mut scratch, &mut p_next)
        .expect("parent eval");
    let c_out = child_ctrl
        .evaluate_recurrent(&inputs, &state, &mut scratch, &mut c_next)
        .expect("child eval");
    assert_eq!(p_out.map(f32::to_bits), c_out.map(f32::to_bits));
    assert_eq!(p_next.map(f32::to_bits), c_next.map(f32::to_bits));
}

/// FL-054 registry evidence: stateful replay across checkpoints (`Held` v2),
/// worker counts, and `Compact` vs `FreeList` arrangements; bounded recurrent
/// state in `[-1, 1]^S`; zero leakage to newborns; exact parameter-count
/// energy costing vs stateless baseline; and watched activation trace parity.
#[test]
fn recurrent_world_stateful_replay_checkpoint_and_energy_costing() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let dense_cfg = validate(base_config(None)).expect("dense cfg");
    let rec_cfg = validate(base_config(Some(8))).expect("recurrent cfg");

    // 1. Verify energy costing (plan §25.2): recurrent controller maintenance
    //    cost scales strictly with the extra recurrent parameters `P_rec / P_dense`.
    let mut dense_world = World::found(&dense_cfg, Arrangement::Compact, profile).expect("dense");
    let mut rec_world = World::found(&rec_cfg, Arrangement::Compact, profile).expect("rec");
    assert_eq!(rec_world.recurrent_state_width(), 8);

    // Bounded memory admission succeeds and stays fixed across ticks.
    let admission = rec_world.admit_memory(false, 0, 0).expect("admit");
    assert!(admission.peak_bytes <= admission.budget_bytes);
    let footprint_before = rec_world.footprint();

    let watched_id = rec_world.population().iter().next().expect("founder").id;
    dense_world.watch(Some(watched_id));
    rec_world.watch(Some(watched_id));
    step_world(&mut dense_world);
    step_world(&mut rec_world);

    let dense_ctrl_cost = dense_world
        .watched_transactions()
        .last()
        .and_then(|t| t.maintenance)
        .expect("dense maintenance")
        .paid
        .controller;
    let rec_ctrl_cost = rec_world
        .watched_transactions()
        .last()
        .and_then(|t| t.maintenance)
        .expect("rec maintenance")
        .paid
        .controller;
    let expected_ratio = rec_cfg.report().default_parameter_count as f64
        / dense_cfg.report().default_parameter_count as f64;
    assert!(
        rec_ctrl_cost > dense_ctrl_cost
            && ((rec_ctrl_cost / dense_ctrl_cost) - expected_ratio).abs() < 1e-12,
        "recurrent controller cost must scale with parameter count: rec={rec_ctrl_cost:e} \
         dense={dense_ctrl_cost:e} expected_ratio={expected_ratio:e}"
    );

    // Watched organism snapshot traces recurrent hidden activations cleanly.
    let snap = rec_world
        .organism_snapshot(watched_id)
        .expect("snapshot")
        .expect("alive");
    assert!(snap.activations.is_some());
    assert_eq!(snap.organism.held.recurrent_state().len(), 8);
    assert!(
        snap.organism
            .held
            .recurrent_state()
            .iter()
            .any(|&v| v != 0.0),
        "recurrent state should become non-zero after first evaluation"
    );

    // 2. Full stateful replay: Compact vs FreeList vs 4-worker vs mid-run checkpoint.
    let mut compact = World::found(&rec_cfg, Arrangement::Compact, profile).expect("compact");
    let mut freelist = World::found(&rec_cfg, Arrangement::FreeList, profile).expect("freelist");
    let mut parallel = World::found(&rec_cfg, Arrangement::Compact, profile).expect("parallel");
    parallel
        .set_cpu_workers(NonZeroUsize::new(4).expect("4"))
        .expect("4 workers");

    let mut saw_birth = false;
    let mut saw_held_state_preserved = false;
    for step in 0..45_u32 {
        let before_states: Vec<_> = compact
            .population()
            .iter()
            .map(|o| {
                (
                    o.id,
                    o.held.evaluated_at(),
                    o.held.recurrent_state().to_vec(),
                )
            })
            .collect();
        let rep = step_world(&mut compact);
        step_world(&mut freelist);
        step_world(&mut parallel);

        assert_eq!(compact.digest(), freelist.digest(), "C.7 at step {step}");
        assert_eq!(
            compact.digest(),
            parallel.digest(),
            "workers at step {step}"
        );

        // Every newborn starts with empty/zero recurrent state (`HeldOutput::INVALID`),
        // and non-evaluated survivors keep their exact recurrent state bits.
        for born in &rep.births {
            saw_birth = true;
            let child = compact.population().get(born.child).expect("child");
            assert!(!child.held.is_valid());
            assert!(child.held.recurrent_state().is_empty());
        }
        for (id, prev_at, prev_state) in &before_states {
            if let Some(after) = compact.population().get(*id) {
                for &v in after.held.recurrent_state() {
                    assert!(v.is_finite() && (-1.0..=1.0).contains(&v));
                }
                if after.held.evaluated_at() == *prev_at && !prev_state.is_empty() {
                    assert_eq!(
                        after.held.recurrent_state(),
                        prev_state.as_slice(),
                        "held tick must preserve recurrent state bit-for-bit"
                    );
                    saw_held_state_preserved = true;
                }
            }
        }
    }
    assert!(
        saw_birth,
        "expected at least one birth to test newborn zero state"
    );
    assert!(
        saw_held_state_preserved,
        "expected non-evaluation ticks to preserve recurrent state"
    );

    // 3. Checkpoint write/restore mid-run (`Held` v2) replays bit-for-bit identically.
    let mut writer = CheckpointWriter::new(&rec_cfg.config().model);
    compact.write_checkpoint(&mut writer).expect("write");
    let bytes = writer.finish().expect("finish");
    let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("parse");
    let mut restored = World::read_checkpoint(&reader, profile).expect("read recurrent");
    assert_eq!(compact.digest(), restored.digest());

    for step in 0..20_u32 {
        step_world(&mut compact);
        step_world(&mut restored);
        assert_eq!(
            compact.digest(),
            restored.digest(),
            "checkpoint replay diverged at post-restore step {step}"
        );
    }

    // Bounded footprint never grew during live stepping.
    let footprint_after = rec_world.footprint();
    assert_eq!(footprint_after.field, footprint_before.field);
    assert_eq!(footprint_after.scratch, footprint_before.scratch);
    assert_eq!(footprint_after.staging, footprint_before.staging);
    assert!(footprint_after.state <= footprint_before.state);
}

#[derive(Debug)]
struct StubMlpControllerBackend;

impl ControllerBackend for StubMlpControllerBackend {
    fn describe(&self) -> String {
        "stub-mlp-only-controller".to_owned()
    }

    fn resident_bytes(&self) -> u64 {
        0
    }

    fn forget(&mut self) {}

    fn evaluate(
        &mut self,
        _jobs: &[ControllerJob<'_>],
        _outputs: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
    ) -> Result<(), CoreError> {
        panic!("MLP-only controller backend must never be dispatched on a recurrent world");
    }
}

#[test]
fn mlp_only_controller_backend_refuses_recurrent_world_honestly() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let rec_cfg = validate(base_config(Some(4))).expect("valid");
    let mut reference = World::found(&rec_cfg, Arrangement::Compact, profile).expect("ref");
    let mut with_backend =
        World::found(&rec_cfg, Arrangement::Compact, profile).expect("with backend");
    with_backend.set_controller_backend(Some(Box::new(StubMlpControllerBackend)));

    step_world(&mut reference);
    step_world(&mut with_backend);

    let report = with_backend.controller_report();
    assert_eq!(report.batched, 0);
    assert_eq!(report.reference_ticks, 1);
    assert!(
        report
            .failure
            .as_deref()
            .is_some_and(|msg| msg.contains(INDEPENDENT_RECURRENT_MLP_V1)),
        "expected honest refusal naming recurrent family, got {:?}",
        report.failure
    );
    assert_eq!(reference.digest(), with_backend.digest());
}
