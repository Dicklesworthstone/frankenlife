//! FL-047 evidence (plan §7.5–§7.6, §16.3–§16.7, §21.3 Gate G8): endurance
//! turnover, bounded post-warmup memory, multi-stage checkpoint/restart
//! recovery, and explicit fault-injection subruns.
//!
//! Covers every functional Gate G8 requirement outside the multi-hour wall
//! soak:
//! 1. Controlled-churn endurance fixture with genuine births, deaths, and slot
//!    reclamation across hundreds of ticks: verifies `INV-01`, `INV-02`,
//!    `INV-03` ledger closure, `INV-07` dead-identity non-resurrection, zero
//!    hidden reseeding, and bounded post-warmup memory footprint (no leak
//!    slope across repeated allocation/reclamation cycles).
//! 2. Multi-stage checkpoint -> drop -> restore -> continue across real
//!    lifecycle churn (`pause-and-stream`), verifying exact bit-for-bit
//!    `WorldDigest` identity with an uninterrupted control run AND verifying
//!    that damaged/corrupted checkpoint chunks are caught and rejected cleanly
//!    (falling back to the last intact verified checkpoint).
//! 3. Explicit capacity-limit (`pause-before-commit`) fault subrun verifying
//!    that a population ceiling halts cleanly (`Stepped::Paused`), preserves
//!    exact committed state (`INV-01`/`INV-03`/`INV-12`), never silently drops
//!    births, and resumes cleanly after an explicit removal intervention.
//! 4. Explicit numerical-fault (`stop-and-preserve`) subrun verifying that
//!    invalid non-finite inputs are rejected cleanly and NEVER masquerade as
//!    organism death (plan §7.6).

#![forbid(unsafe_code)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::cast_precision_loss,
    reason = "integration tests: unexpected failure is a test failure; observed metrics printed"
)]

use std::collections::HashSet;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use flife_cli::{
    CheckpointMode, ControllerChoice, Persistence, Record, ResumeOptions, RunOptions,
    StoreDurability, Until, resume, run_with,
};
use flife_core::ids::OrganismId;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{
    Arrangement, Disposition, Intervention, NumericalProfile, STOP_AND_PRESERVE, Stepped, World,
};

const REFERENCE: &str = include_str!("../../../presets/reference-small-world.toml");

fn scratch_dir(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("flife-fl047-endurance")
        .join(format!("{label}-{}-{n}", std::process::id()));
    fs::create_dir_all(&dir).expect("create endurance scratch dir");
    dir
}

/// High-churn compact ecology derived cleanly from `reference-small-world.toml`
/// with high birth/death turnover so slot allocation and reclamation are
/// exercised aggressively without hidden reseeding.
fn high_churn_ecology_toml(max_count: u32) -> String {
    let mut text = REFERENCE.to_owned();
    for (from, to) in [
        ("width = 128.0\n", "width = 32.0\n"),
        ("height = 128.0\n", "height = 32.0\n"),
        ("grid_width = 128\n", "grid_width = 32\n"),
        ("grid_height = 128\n", "grid_height = 32\n"),
        ("initial_count = 4096\n", "initial_count = 128\n"),
        (
            "maximum_count = 16384\n",
            &format!("maximum_count = {max_count}\n"),
        ),
        ("initial_energy = 2.0\n", "initial_energy = 2.6\n"),
        ("count = 8\n", "count = 4\n"),
        ("power_per_source = 960.0\n", "power_per_source = 190.0\n"),
        ("sigma_world_units = 10.0\n", "sigma_world_units = 3.2\n"),
        (
            "path_direction_interval_seconds = 30.0\n",
            "path_direction_interval_seconds = 2.0\n",
        ),
        (
            "uptake_capacity_per_second = 6.0\n",
            "uptake_capacity_per_second = 9.0\n",
        ),
        (
            "basal_energy_per_second = 0.8\n",
            "basal_energy_per_second = 1.15\n",
        ),
        (
            "minimum_parent_energy_before_attempt = 3.0\n",
            "minimum_parent_energy_before_attempt = 2.2\n",
        ),
        (
            "maximum_attempt_rate_per_second = 1.0\n",
            "maximum_attempt_rate_per_second = 6.0\n",
        ),
        ("cooldown_seconds = 0.5\n", "cooldown_seconds = 0.125\n"),
        (
            "evaluation_interval_ticks = 1\n",
            "evaluation_interval_ticks = 2\n",
        ),
        (
            "maximum_logical_genome_bytes = 1073741824\n",
            "maximum_logical_genome_bytes = 134217728\n",
        ),
        (
            "maximum_allocated_genome_bytes = 1207959552\n",
            "maximum_allocated_genome_bytes = 167772160\n",
        ),
        (
            "maximum_total_bytes = 2147483648\n",
            "maximum_total_bytes = 536870912\n",
        ),
    ] {
        assert!(
            text.contains(from),
            "reference preset missing line {from:?}"
        );
        text = text.replacen(from, to, 1);
    }
    text
}

/// Gate G8 controlled-churn endurance: exercises genuine births, deaths, and
/// arena slot reclamation across 512 ticks while verifying:
/// - `INV-01` & `INV-03` conservation at every tick;
/// - `INV-07` dead identities never reappear when slots are reused;
/// - Post-warmup memory footprint stays strictly bounded (no runaway drift);
/// - Distinct wall clock vs simulated clock reported honestly.
#[test]
fn controlled_churn_endurance_turnover_bounded_memory_and_inv07() {
    let config =
        ValidatedConfig::from_toml(&high_churn_ecology_toml(512)).expect("valid churn config");
    assert!(!config.config().population.automatic_reseed);
    let profile = NumericalProfile::reference_f32().expect("reference profile");
    let workers = NonZeroUsize::new(4).expect("non-zero");
    let mut world =
        World::found_on(&config, Arrangement::FreeList, profile, workers).expect("found");

    let ticks = 512_u64;
    let warmup_tick = 128_u64;
    let t0 = Instant::now();

    let mut total_births = 0_u64;
    let mut total_deaths = 0_u64;
    let mut seen_dead: HashSet<OrganismId> = HashSet::new();
    let mut prev_live: HashSet<OrganismId> = world.population().ids().collect();
    let mut post_warmup_footprint_min = u64::MAX;
    let mut post_warmup_footprint_max = 0_u64;
    let mut max_abs_residual = 0.0_f64;

    for t in 1..=ticks {
        let report = match world.step().expect("step succeeds") {
            Stepped::Published(r) => *r,
            Stepped::Paused(shortfall) => {
                panic!("unexpected capacity pause at tick {t}: {shortfall:?}")
            }
        };
        total_births += u64::try_from(report.births.len()).expect("u64");
        total_deaths += u64::try_from(report.deaths.len()).expect("u64");
        let norm_res = report.ledger.normalized.unwrap_or(0.0).abs();
        max_abs_residual = max_abs_residual.max(norm_res);
        assert!(world.fault().is_none(), "unexpected fault at tick {t}");
        // Plan §21.4 canonical INV-03 tolerance: |residual| / max(initial, turnover) <= 1e-5.
        assert!(
            norm_res <= 1.0e-5,
            "INV-03 normalized residual {norm_res} exceeded 1e-5 at tick {t}"
        );

        // Sample live IDs every 16 ticks to prove INV-07 (dead identity never resurrected).
        if t % 16 == 0 {
            let current_live: HashSet<OrganismId> = world.population().ids().collect();
            for &id in &prev_live {
                if !current_live.contains(&id) {
                    seen_dead.insert(id);
                }
            }
            for &id in &current_live {
                assert!(
                    !seen_dead.contains(&id),
                    "INV-07 VIOLATION: dead identity {id:?} resurrected at tick {t}"
                );
            }
            prev_live = current_live;
        }

        if t >= warmup_tick && t % 32 == 0 {
            let fp = world.footprint().total();
            post_warmup_footprint_min = post_warmup_footprint_min.min(fp);
            post_warmup_footprint_max = post_warmup_footprint_max.max(fp);
        }
    }

    let wall_s = t0.elapsed().as_secs_f64();
    let dt = f64::from(config.config().world.tick_numerator)
        / f64::from(config.config().world.tick_denominator);
    let sim_s = (ticks as f64) * dt;

    println!(
        "[FL-047 G8 Churn] ticks={ticks} sim_s={sim_s:.2} wall_s={wall_s:.2} ({:.1} t/s) \
         births={total_births} deaths={total_deaths} live_end={} max_|INV-03|={max_abs_residual:.3e} \
         post_warmup_footprint=[{post_warmup_footprint_min}, {post_warmup_footprint_max}]",
        (ticks as f64) / wall_s.max(1e-6),
        world.population().len()
    );

    assert!(
        total_births > 0 && total_deaths > 0,
        "G8 controlled-churn fixture must observe both births ({total_births}) and deaths ({total_deaths})"
    );
    let max_budget = config.config().execution.maximum_total_bytes;
    assert!(
        post_warmup_footprint_max <= max_budget,
        "footprint {post_warmup_footprint_max} exceeded budget {max_budget}"
    );
    // Repeated reclamation must keep post-warmup footprint strictly bounded
    // (within +2 MiB of the post-warmup floor, catching leaks well below budget).
    assert!(
        post_warmup_footprint_min > 0 && post_warmup_footprint_max >= post_warmup_footprint_min,
        "invalid post-warmup footprint range [{post_warmup_footprint_min}, {post_warmup_footprint_max}]"
    );
    assert!(
        post_warmup_footprint_max <= post_warmup_footprint_min.saturating_add(2 << 20),
        "post-warmup footprint grew beyond 2 MiB bound: min={post_warmup_footprint_min} max={post_warmup_footprint_max}"
    );
}

/// Gate G8 checkpoint/restart + corrupted-checkpoint recovery:
/// 1. Runs an uninterrupted control for 192 ticks with real births & deaths.
/// 2. Runs a persisted run directory in two stages (`0 -> 96`, then `resume 96 -> 192`),
///    and ALSO tests corrupting the newest checkpoint chunk on disk and verifying that
///    `resume` detects the corruption, skips the damaged checkpoint, and still recovers
///    to the EXACT bit-for-bit `WorldDigest` of the uninterrupted control!
#[test]
fn multi_stage_checkpoint_restart_and_corrupted_chunk_recovery() {
    let config =
        ValidatedConfig::from_toml(&high_churn_ecology_toml(512)).expect("valid churn config");
    let workers = NonZeroUsize::new(4).expect("non-zero");

    // 1. Uninterrupted control run (0 -> 192 ticks)
    fn capture_end() -> (
        std::sync::Arc<std::sync::Mutex<String>>,
        impl FnMut(&Record) + Send + 'static,
    ) {
        let slot = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let slot_c = std::sync::Arc::clone(&slot);
        let cb = move |record: &Record| {
            if let Record::End { digest, .. } = record {
                *slot_c.lock().expect("lock") = digest.clone();
            }
        };
        (slot, cb)
    }

    let control_opts = RunOptions {
        ticks: 192,
        every: 64,
        metrics_every: 0,
        cpu_workers: Some(workers),
        controllers: ControllerChoice::Cpu,
    };
    let (ctrl_slot, ctrl_cb) = capture_end();
    let _ = run_with(&config, control_opts, &[], None, ctrl_cb).expect("control run succeeds");
    let control_digest = ctrl_slot.lock().expect("lock").clone();
    assert!(!control_digest.is_empty());

    // 2. Stage A: run 0 -> 96 with pause-and-stream checkpoints every 32 ticks
    let run_dir = scratch_dir("restart-stage");
    let persist = Persistence {
        run_dir: run_dir.clone(),
        checkpoint_every: 32,
        durability: StoreDurability::ProcessCrash,
        mode: CheckpointMode::PauseAndStream,
        retained_bytes: config.config().execution.checkpoint_retained_bytes,
    };
    let stage_a = RunOptions {
        ticks: 96,
        every: 48,
        metrics_every: 0,
        cpu_workers: Some(workers),
        controllers: ControllerChoice::Cpu,
    };
    let (a_slot, a_cb) = capture_end();
    let _ = run_with(&config, stage_a, &[], Some(&persist), a_cb).expect("stage A run succeeds");
    assert!(!a_slot.lock().expect("lock").is_empty());

    // 3. Stage B: resume cleanly 96 -> 192 and verify bit-exact digest parity
    let resume_opts = ResumeOptions {
        until: Until::Tick(192),
        every: 48,
        metrics_every: 0,
        cpu_workers: Some(workers),
        controllers: ControllerChoice::Cpu,
        durability: StoreDurability::ProcessCrash,
        checkpoint_every: Some(32),
        checkpoint_mode: Some(CheckpointMode::PauseAndStream),
        retained_bytes: None,
    };
    let (b_slot, b_cb) = capture_end();
    let _ = resume(&run_dir, &resume_opts, b_cb).expect("stage B resume succeeds");
    let stage_b_digest = b_slot.lock().expect("lock").clone();
    assert_eq!(
        stage_b_digest, control_digest,
        "restarted run must match uninterrupted control bit-for-bit"
    );

    // 4. Fault injection: corrupt the highest-sequence checkpoint manifest under
    //    `store/checkpoints/<sequence>.manifest` (sorting numerically, never
    //    lexicographically so `10.manifest` > `2.manifest`) and confirm `resume`
    //    detects the corruption, skips the damaged checkpoint, and recovers to
    //    the exact uninterrupted digest!
    let ckpt_dir = run_dir.join("store").join("checkpoints");
    let mut sequenced: Vec<(u64, PathBuf)> = fs::read_dir(&ckpt_dir)
        .expect("read checkpoints dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter_map(|p| {
            let seq = p.file_stem()?.to_str()?.parse::<u64>().ok()?;
            Some((seq, p))
        })
        .collect();
    sequenced.sort_by_key(|(seq, _)| *seq);
    assert!(
        sequenced.len() >= 2,
        "expected at least two persisted checkpoint manifests under {}",
        ckpt_dir.display()
    );
    let (_, newest_chunk) = sequenced.last().expect("at least one checkpoint manifest");
    let mut bytes = fs::read(newest_chunk).expect("read newest checkpoint file");
    assert!(bytes.len() > 32, "checkpoint file non-trivial");
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    fs::write(newest_chunk, bytes).expect("corrupt newest checkpoint");

    let skipped_count = std::sync::Arc::new(AtomicU64::new(0));
    let skipped_clone = std::sync::Arc::clone(&skipped_count);
    let rec_slot = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let rec_slot_c = std::sync::Arc::clone(&rec_slot);
    let _ = resume(&run_dir, &resume_opts, move |record| match record {
        Record::Resume { skipped, .. } => {
            skipped_clone.store(skipped.len() as u64, Ordering::Relaxed);
        }
        Record::End { digest, .. } => {
            *rec_slot_c.lock().expect("lock") = digest.clone();
        }
        _ => {}
    })
    .expect("resume recovers cleanly from damaged newest checkpoint");
    assert!(
        skipped_count.load(Ordering::Relaxed) >= 1,
        "expected resume to detect and skip the corrupted newest checkpoint"
    );
    assert_eq!(
        *rec_slot.lock().expect("lock"),
        control_digest,
        "recovery after corrupted checkpoint must still match uninterrupted control bit-for-bit"
    );
}

/// Gate G8 explicit capacity-limit & numerical-fault subruns (plan §7.5–§7.6):
/// 1. `pause-before-commit` halts cleanly when natural biological births hit
///    `maximum_count` without corrupting committed state or silently dropping
///    births, then resumes cleanly after explicit organism removal (`INV-12`).
/// 2. Invalid non-finite intervention input is rejected cleanly without altering
///    committed state or killing organisms (`stop-and-preserve` honesty).
#[test]
fn capacity_pause_and_numerical_fault_subruns_preserve_committed_state() {
    // Ceiling equals founder count (128) with fertile initial energy (4.5 > 2.2)
    // so natural biological births hit `pause-before-commit` immediately!
    let tight_toml = high_churn_ecology_toml(128)
        .replacen("initial_energy = 2.6\n", "initial_energy = 4.5\n", 1)
        .replacen(
            "maximum_attempt_rate_per_second = 6.0\n",
            "maximum_attempt_rate_per_second = 64.0\n",
            1,
        );
    let config = ValidatedConfig::from_toml(&tight_toml).expect("valid tight cap config");
    assert_eq!(
        config.config().population.capacity_policy,
        "pause-before-commit"
    );
    let profile = NumericalProfile::reference_f32().expect("reference profile");
    let workers = NonZeroUsize::new(2).expect("non-zero");
    let mut world =
        World::found_on(&config, Arrangement::FreeList, profile, workers).expect("found");

    // 1. Step until natural biological births trigger explicit `Stepped::Paused`.
    let mut saw_pause = false;
    for _ in 0..64 {
        let digest_before = world.digest();
        let live_before = world.population().len();
        let tick_before = world.tick();
        match world
            .step()
            .expect("capacity pause is an explicit Stepped::Paused, not a crash")
        {
            Stepped::Published(_) => {}
            Stepped::Paused(shortfall) => {
                assert!(shortfall.required > shortfall.available);
                // Committed world state is completely unchanged on pause-before-commit!
                assert_eq!(world.digest(), digest_before);
                assert_eq!(world.population().len(), live_before);
                assert_eq!(world.tick(), tick_before);
                saw_pause = true;
                break;
            }
        }
    }
    assert!(
        saw_pause,
        "expected natural births at ceiling 128 to trigger explicit pause-before-commit"
    );

    // 2. Explicit removal frees capacity and allows stepping cleanly with exact INV-03 closure.
    let live_paused = world.population().len();
    let after_removal = world
        .step_with(&[Intervention::RemoveOrganisms {
            center: [16.0, 16.0],
            radius: 12.0,
            disposition: Disposition::Recycle,
        }])
        .expect("step after removal succeeds");
    match after_removal {
        Stepped::Published(report) => {
            assert!(report.ledger.normalized.unwrap_or(0.0).abs() <= 1.0e-5);
            assert!(world.population().len() < live_paused);
        }
        Stepped::Paused(s) => panic!("unexpected pause after removal: {s:?}"),
    }

    // 3. Pre-prepare invalid intervention refusal: NaN energy injection must be
    //    rejected cleanly before `prepare()` without latching `world.fault()` or
    //    killing organisms, leaving the world intact and steppable.
    let digest_pre_nan = world.digest();
    let live_pre_nan = world.population().len();
    let tick_pre_nan = world.tick();
    let err = world
        .step_with(&[Intervention::InjectEnergy {
            cell: [8, 8],
            amount: f64::NAN,
        }])
        .expect_err("NaN injection must be refused");
    assert!(
        matches!(err, CoreError::NumericalInvalid { .. }),
        "expected NumericalInvalid for NaN intervention, got {err:?}"
    );
    assert!(
        world.fault().is_none(),
        "pre-prepare invalid command must not latch a stop-and-preserve run fault"
    );
    assert_eq!(
        world.digest(),
        digest_pre_nan,
        "committed world must stay intact after NaN refusal"
    );
    assert_eq!(
        world.population().len(),
        live_pre_nan,
        "invalid command must never kill organisms"
    );
    assert_eq!(world.tick(), tick_pre_nan);

    // 4. True mid-step `stop-and-preserve` numerical fault (plan §7.6): found a
    //    world whose enormous narrow source (`power_per_source = 1.0e40`,
    //    `sigma = 0.5`) overflows the F32 field during `World::prepare()` after
    //    publishing valid ticks, proving:
    //    - `CoreError::NumericalInvalid` is returned;
    //    - `world.fault()` latches with the exact input tick and error;
    //    - committed digest, live organism count, and tick are preserved bit-for-bit
    //      (numerical fault NEVER masquerades as organism death);
    //    - subsequent `world.step()` is refused with `StateConflict` naming
    //      `stop-and-preserve`.
    let overflow_toml = high_churn_ecology_toml(512)
        .replacen("count = 4\n", "count = 1\n", 1)
        .replacen(
            "power_per_source = 190.0\n",
            "power_per_source = 1.0e40\n",
            1,
        )
        .replacen("sigma_world_units = 3.2\n", "sigma_world_units = 0.5\n", 1);
    let overflow_config =
        ValidatedConfig::from_toml(&overflow_toml).expect("valid overflow config");
    let mut overflow_world =
        World::found_on(&overflow_config, Arrangement::FreeList, profile, workers)
            .expect("found overflow world");
    let (overflow_err, digest_pre_fault, live_pre_fault, tick_pre_fault) = loop {
        assert!(
            overflow_world.tick().get() < 64,
            "expected F32 field overflow within 64 ticks"
        );
        let d_before = overflow_world.digest();
        let l_before = overflow_world.population().len();
        let t_before = overflow_world.tick();
        match overflow_world.step() {
            Ok(Stepped::Published(_)) => {}
            Ok(Stepped::Paused(s)) => panic!("unexpected pause before overflow: {s:?}"),
            Err(err) => break (err, d_before, l_before, t_before),
        }
    };
    assert!(
        matches!(overflow_err, CoreError::NumericalInvalid { .. }),
        "expected NumericalInvalid on F32 field overflow, got {overflow_err:?}"
    );
    let fault = overflow_world
        .fault()
        .expect("mid-step NumericalInvalid must latch stop-and-preserve fault");
    assert_eq!(fault.tick, tick_pre_fault);
    assert_eq!(fault.error, overflow_err);
    assert_eq!(
        overflow_world.digest(),
        digest_pre_fault,
        "stop-and-preserve must preserve committed world bit-for-bit"
    );
    assert_eq!(
        overflow_world.population().len(),
        live_pre_fault,
        "mid-step numerical fault must never masquerade as organism death"
    );
    assert_eq!(overflow_world.tick(), tick_pre_fault);

    let after_fault = overflow_world
        .step()
        .expect_err("stopped world must refuse subsequent steps");
    assert!(
        matches!(&after_fault, CoreError::StateConflict { detail, .. } if detail.contains(STOP_AND_PRESERVE)),
        "expected StateConflict naming {STOP_AND_PRESERVE}, got {after_fault:?}"
    );
    assert_eq!(overflow_world.digest(), digest_pre_fault);
}
