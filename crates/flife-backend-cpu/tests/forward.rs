//! The CPU backend against the reference controller, organism by organism
//! and bit for bit, over every catalog topology in one mixed batch, on one
//! lane and on several, in shards of any size and at slots far past 32 bits;
//! residency by arena slot; and the executor: it needs no rayon worker.

use std::num::NonZeroUsize;
use std::sync::{Arc, Barrier, mpsc};
use std::time::Duration;

use flife_backend_cpu::{CpuBackend, MINIMUM_JOBS_PER_LANE};
use flife_core::CoreError;
use flife_core::binary16::F16;
use flife_core::ids::GenomeId;
use flife_neural::{
    ACTION_WIDTH, ArenaSlot, BackendBudget, ControllerBackend, ControllerJob, ControllerScratch,
    DenseTopology, FlatControllerJob, FlatOutputs, RawOutputs, ReferenceController, SENSOR_WIDTH,
};
use rayon::prelude::*;

/// The `dense-104-36-v1` catalog's hidden widths.
const TOPOLOGIES: [&[u32]; 5] = [&[8], &[32], &[64, 32], &[96, 64], &[128, 96]];

/// A bound admitting every finite F32 parameter.
const BOUND: f64 = f32::MAX as f64;

/// SplitMix64, and uniform draws from it.
struct Draws(u64);

impl Draws {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[-1, 1)`.
    fn symmetric(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1_u64 << 52) as f64 - 1.0
    }
}

/// How an organism's parameters and inputs are drawn.
#[derive(Debug, Clone, Copy)]
enum Regime {
    /// Fan-in-scaled weights and observation-sized inputs.
    Typical,
    /// Weights eight times larger: many activations saturate.
    Saturating,
    /// Subnormal inputs, unit weights and zero biases: subnormal products
    /// and sums, which the CPU keeps as the reference does.
    Tiny,
    /// Hidden pre-activations overflow F32.
    HiddenOverflow,
    /// Only the linear outputs overflow.
    OutputOverflow,
    /// One input is finite in F64 and infinite in F32.
    WideInput,
}

fn parameters(topology: &DenseTopology, regime: Regime, draws: &mut Draws) -> Vec<f32> {
    let mut values = vec![0.0_f32; topology.parameter_count()];
    let last = topology.layers().len() - 1;
    for (index, layer) in topology.layers().iter().enumerate() {
        let scale = match regime {
            Regime::Typical | Regime::WideInput => 2.0 / (layer.inputs as f64).sqrt(),
            Regime::Saturating => 16.0 / (layer.inputs as f64).sqrt(),
            Regime::Tiny => 1.0,
            Regime::HiddenOverflow if index == 0 => 1.0e30,
            Regime::OutputOverflow if index == last => 3.0e38,
            Regime::HiddenOverflow | Regime::OutputOverflow => 1.0,
        };
        let end = layer.biases + layer.outputs;
        for value in &mut values[layer.weights..end] {
            *value = (draws.symmetric() * scale) as f32;
        }
        if matches!(regime, Regime::Tiny) {
            values[layer.biases..end].fill(0.0);
        }
        if matches!(regime, Regime::OutputOverflow) && index == last {
            for value in &mut values[layer.weights..end] {
                *value = value.abs();
            }
        }
    }
    values
}

fn inputs(regime: Regime, draws: &mut Draws) -> [f64; SENSOR_WIDTH] {
    let mut inputs = [0.0_f64; SENSOR_WIDTH];
    for (index, input) in inputs.iter_mut().enumerate() {
        *input = match (regime, index % 7) {
            // Exact zeros, as an empty channel gives.
            (_, 0) if draws.next().is_multiple_of(3) => 0.0,
            (Regime::Tiny, _) => draws.symmetric() * 1.0e-39,
            (Regime::HiddenOverflow, _) => draws.symmetric().abs() * 1.0e10 + 1.0,
            // Unnarrowed F64 values: rounding to F32 is part of the contract.
            (_, 1) => draws.symmetric() * 40.0,
            _ => draws.symmetric() * 2.0,
        };
    }
    if matches!(regime, Regime::WideInput) {
        inputs[(draws.next() % SENSOR_WIDTH as u64) as usize] = 1.0e39;
    }
    inputs
}

struct Case {
    topology: usize,
    regime: Regime,
    parameters: Vec<f32>,
    inputs: [f64; SENSOR_WIDTH],
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure is a test failure"
)]
fn topologies() -> Vec<DenseTopology> {
    TOPOLOGIES
        .iter()
        .map(|widths| DenseTopology::new(widths))
        .collect::<Result<_, _>>()
        .expect("the catalog's topologies")
}

fn cases(count: usize, seed: u64) -> Vec<Case> {
    let catalog = topologies();
    let mut draws = Draws(seed);
    (0..count)
        .map(|index| {
            let topology = (draws.next() % 5) as usize;
            let regime = match index % 20 {
                0..=11 => Regime::Typical,
                12..=14 => Regime::Saturating,
                15 | 16 => Regime::Tiny,
                17 => Regime::HiddenOverflow,
                18 => Regime::OutputOverflow,
                _ => Regime::WideInput,
            };
            Case {
                topology,
                regime,
                parameters: parameters(&catalog[topology], regime, &mut draws),
                inputs: inputs(regime, &mut draws),
            }
        })
        .collect()
}

fn jobs<'a>(topologies: &'a [DenseTopology], cases: &'a [Case]) -> Vec<ControllerJob<'a>> {
    jobs_from(topologies, cases, 0)
}

/// As [`jobs`], each topology's slots numbered from `first` in case order.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure is a test failure"
)]
fn jobs_from<'a>(
    topologies: &'a [DenseTopology],
    cases: &'a [Case],
    first: u64,
) -> Vec<ControllerJob<'a>> {
    let mut next = [first; TOPOLOGIES.len()];
    cases
        .iter()
        .enumerate()
        .map(|(index, case)| ControllerJob {
            genome: GenomeId::new(index as u64 + 1),
            slot: {
                let slot = next[case.topology];
                next[case.topology] += 1;
                ArenaSlot::new(case.topology as u32, slot, 1)
            },
            controller: ReferenceController::new(
                &topologies[case.topology],
                &case.parameters,
                BOUND,
            )
            .expect("an admitted controller"),
            inputs: &case.inputs,
        })
        .collect()
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure is a test failure"
)]
fn lanes(count: usize) -> NonZeroUsize {
    NonZeroUsize::new(count).expect("nonzero")
}

type Outputs = Vec<Option<RawOutputs>>;

fn bits(outputs: &Outputs) -> Vec<Option<Vec<u32>>> {
    outputs
        .iter()
        .map(|output| output.map(|raw| raw.iter().map(|v| v.to_bits()).collect()))
        .collect()
}

#[test]
fn every_topology_matches_the_reference_bit_for_bit_on_one_lane_and_on_many()
-> Result<(), CoreError> {
    let topologies = topologies();
    let cases = cases(2_000, 2026);
    let jobs = jobs(&topologies, &cases);

    let mut one = CpuBackend::new(lanes(1));
    let mut outputs = Vec::new();
    one.evaluate(&jobs, &mut outputs)?;
    assert_eq!(outputs.len(), cases.len());

    let (mut refused, mut tiny) = ([0_usize; 3], 0_usize);
    for (index, ((case, job), got)) in cases.iter().zip(&jobs).zip(&outputs).enumerate() {
        let mut scratch = ControllerScratch::new(&topologies[case.topology])?;
        match (job.controller.evaluate(&case.inputs, &mut scratch), got) {
            (Ok(want), Some(got)) => {
                let differs = (0..ACTION_WIDTH).find(|&k| want[k].to_bits() != got[k].to_bits());
                assert!(
                    differs.is_none(),
                    "organism {index} ({:?}, {:?}): output {differs:?} is {:?}, not {:?}",
                    TOPOLOGIES[case.topology],
                    case.regime,
                    differs.map(|k| got[k]),
                    differs.map(|k| want[k]),
                );
                tiny += usize::from(matches!(case.regime, Regime::Tiny));
            }
            (Err(CoreError::NumericalInvalid { quantity, .. }), None) => {
                let kind = match quantity {
                    "controller input" => 0,
                    "controller pre-activation" => 1,
                    _ => 2,
                };
                refused[kind] += 1;
            }
            (want, got) => panic!(
                "organism {index} ({:?}): reference {want:?}, CPU backend {got:?}",
                case.regime
            ),
        }
    }
    // Every refusal was exercised, and subnormal arithmetic was evaluated,
    // not deferred, so agreement on them is not vacuous.
    assert!(
        refused.iter().all(|&count| count > 0),
        "refusals {refused:?}"
    );
    assert!(tiny > 0, "no subnormal organism was evaluated");

    // On several lanes the outputs are the same bits.
    assert!(cases.len() >= 4 * MINIMUM_JOBS_PER_LANE);
    for count in [2, 4, 16] {
        let mut many = CpuBackend::new(lanes(count));
        let mut split = Vec::new();
        many.evaluate(&jobs, &mut split)?;
        assert_eq!(bits(&split), bits(&outputs), "{count} lanes");
    }

    // In shards of one, two or three slots, and with every slot far past
    // 32 bits, they are the same bits too.
    for slots in [1, 2, 3] {
        let mut sharded = CpuBackend::with_shard_slots(lanes(4), slots);
        let mut split = Vec::new();
        sharded.evaluate(&jobs, &mut split)?;
        assert_eq!(bits(&split), bits(&outputs), "{slots} slots to a shard");
        assert!(sharded.shards() * slots as usize >= cases.len());
    }
    for first in [u64::from(u32::MAX) - 7, 1 << 40] {
        let far = jobs_from(&topologies, &cases, first);
        let mut backend = CpuBackend::new(lanes(4));
        let mut split = Vec::new();
        backend.evaluate(&far, &mut split)?;
        assert_eq!(bits(&split), bits(&outputs), "slots from {first}");
    }
    Ok(())
}

/// FL-030: the mixed batch with every parameter rounded once to binary16
/// (compact F16), binary16 subnormals included: each catalog topology's
/// outputs are the reference's bit for bit on one lane and on four, and an
/// input infinite in F32 is still refused. The two regimes whose weights
/// overflow binary16 are left out.
#[test]
fn every_topology_in_f16_matches_the_reference_bit_for_bit() -> Result<(), CoreError> {
    let topologies = topologies();
    let cases: Vec<Case> = cases(2_000, 2027)
        .into_iter()
        .filter(|case| !matches!(case.regime, Regime::HiddenOverflow | Regime::OutputOverflow))
        .collect();
    let halves: Vec<Vec<F16>> = cases
        .iter()
        .map(|case| {
            case.parameters
                .iter()
                .map(|&value| F16::from_f64(f64::from(value)))
                .collect()
        })
        .collect();
    let subnormal = |half: &F16| half.to_bits() & 0x7C00 == 0 && half.to_bits() & 0x03FF != 0;
    assert!(
        halves.iter().flatten().any(subnormal),
        "no binary16 subnormal"
    );
    let mut next = [0_u64; TOPOLOGIES.len()];
    let mut jobs = Vec::with_capacity(cases.len());
    for (index, (case, half)) in cases.iter().zip(&halves).enumerate() {
        let slot = next[case.topology];
        next[case.topology] += 1;
        jobs.push(ControllerJob {
            genome: GenomeId::new(index as u64 + 1),
            slot: ArenaSlot::new(case.topology as u32, slot, 1),
            controller: ReferenceController::new(&topologies[case.topology], half, BOUND)?,
            inputs: &case.inputs,
        });
    }
    let mut want = Vec::with_capacity(jobs.len());
    for job in &jobs {
        let mut scratch = ControllerScratch::new(job.controller.topology())?;
        want.push(
            job.controller
                .evaluate_raw(job.inputs, &mut scratch)
                .ok()
                .map(|raw| raw.iter().map(|v| v.to_bits()).collect::<Vec<_>>()),
        );
    }
    for count in [1, 4] {
        let mut backend = CpuBackend::new(lanes(count));
        let mut outputs = Vec::new();
        backend.evaluate(&jobs, &mut outputs)?;
        assert_eq!(bits(&outputs), want, "{count} lanes");
    }
    let mut evaluated = [0_usize; TOPOLOGIES.len()];
    for (case, want) in cases.iter().zip(&want) {
        evaluated[case.topology] += usize::from(want.is_some());
    }
    assert!(evaluated.iter().all(|&count| count > 0), "{evaluated:?}");
    assert!(want.iter().any(Option::is_none), "no refusal was exercised");
    Ok(())
}

/// A genome's parameters are packed once while it lives, whatever the
/// batches between; a later occupant of its slot is packed in its place; a
/// job is evaluated only from its own generation's and genome's parameters,
/// and its topology's.
#[test]
fn a_genome_is_packed_once_and_a_later_occupant_takes_its_place() -> Result<(), CoreError> {
    let topologies = topologies();
    let cases = cases(300, 7);
    let jobs = jobs(&topologies, &cases);
    let mut backend = CpuBackend::new(lanes(1));
    let mut outputs = Vec::new();
    backend.evaluate(&jobs, &mut outputs)?;
    assert_eq!((backend.resident(), backend.packs()), (300, 300));
    let first = bits(&outputs);

    // Batches of a few: nothing is packed again and nothing is dropped, so a
    // genome due only now and then is not packed each time it is.
    backend.evaluate(&jobs[..100], &mut outputs)?;
    assert_eq!(bits(&outputs), first[..100].to_vec());
    backend.evaluate(&jobs[200..], &mut outputs)?;
    assert_eq!(bits(&outputs), first[200..].to_vec());
    backend.evaluate(&jobs, &mut outputs)?;
    assert_eq!(bits(&outputs), first);
    assert_eq!((backend.resident(), backend.packs()), (300, 300));

    // Genome 1's slot at the next generation: another genome, with other
    // values, packed in its place. It gets its own reference's outputs.
    let mut moved = cases[0].parameters.clone();
    for value in &mut moved {
        *value = -*value;
    }
    let topology = &topologies[cases[0].topology];
    let mut changed = jobs.clone();
    changed[0].genome = GenomeId::new(1_000);
    changed[0].slot = ArenaSlot::new(jobs[0].slot.topology(), jobs[0].slot.slot(), 2);
    changed[0].controller = ReferenceController::new(topology, &moved, BOUND)?;
    backend.evaluate(&changed, &mut outputs)?;
    let mut scratch = ControllerScratch::new(topology)?;
    let want = changed[0]
        .controller
        .evaluate(&cases[0].inputs, &mut scratch)
        .ok()
        .map(|raw| raw.into_iter().map(f32::to_bits).collect::<Vec<_>>());
    assert_eq!(bits(&outputs)[0], want);
    assert_ne!(bits(&outputs)[0], first[0]);
    assert_eq!(bits(&outputs)[1..], first[1..]);
    assert_eq!((backend.resident(), backend.packs()), (300, 301));

    // One batch naming the slot at both generations: the slot holds the last
    // one packed, and the other is deferred, never given its outputs.
    let both = [changed[0], jobs[0]];
    backend.evaluate(&both, &mut outputs)?;
    assert_eq!(outputs[0], None);
    assert_eq!(bits(&outputs)[1], first[0]);

    // The same slot and generation with another genome is another occupant.
    let mut renamed = changed[0];
    renamed.slot = jobs[0].slot;
    backend.evaluate(&[renamed], &mut outputs)?;
    assert_eq!(bits(&outputs)[0], want);

    // A job naming a bucket made for another topology is deferred.
    let other = cases
        .iter()
        .position(|case| case.topology != cases[0].topology)
        .expect("two topologies");
    let mut misfiled = jobs[other];
    misfiled.slot = ArenaSlot::new(jobs[0].slot.topology(), 999, 1);
    backend.evaluate(&[misfiled], &mut outputs)?;
    assert_eq!(outputs, [None]);

    backend.forget();
    assert_eq!((backend.resident(), backend.packs()), (0, 0));
    backend.evaluate(&[], &mut outputs)?;
    assert!(outputs.is_empty());
    Ok(())
}

/// A batch runs on at most the backend's lanes, the calling thread
/// included, and on no more than one thread per `MINIMUM_JOBS_PER_LANE`
/// jobs, whatever the lane count; every split gives the same bits.
#[test]
fn a_batch_runs_on_at_most_its_lanes_and_one_thread_per_minimum_run() -> Result<(), CoreError> {
    let topologies = topologies();
    let cases = cases(2_000, 11);
    let jobs = jobs(&topologies, &cases);
    let mut reference = Vec::new();
    CpuBackend::new(lanes(1)).evaluate(&jobs, &mut reference)?;
    assert_eq!(MINIMUM_JOBS_PER_LANE, 128);
    for (budget, count, threads) in [
        (1, 2_000, 1),
        (4, 2_000, 4),
        (16, 2_000, 16),
        (64, 2_000, 16),
        (16, 300, 3),
        (16, 128, 1),
        (16, 129, 2),
    ] {
        let mut backend = CpuBackend::new(lanes(budget));
        let mut outputs = Vec::new();
        backend.evaluate(&jobs[..count], &mut outputs)?;
        assert_eq!(
            backend.threads_used(),
            threads,
            "{count} jobs on {budget} lanes"
        );
        assert_eq!(bits(&outputs), bits(&reference)[..count].to_vec());
        backend.evaluate(&[], &mut outputs)?;
        assert_eq!(backend.threads_used(), 0);
    }
    Ok(())
}

/// Every worker of rayon's global pool is held while the backend evaluates a
/// batch on four lanes. A backend that handed work to the pool would wait
/// for a worker and never finish; a rayon parallel loop started the same
/// way is the control that shows it would.
#[test]
fn it_evaluates_while_every_rayon_worker_is_busy() {
    let workers = rayon::current_num_threads();
    let release = Arc::new(Barrier::new(workers + 1));
    let (started, held) = mpsc::channel();
    for _ in 0..workers {
        let release = Arc::clone(&release);
        let started = started.clone();
        rayon::spawn(move || {
            let _ = started.send(());
            release.wait();
        });
    }
    for _ in 0..workers {
        held.recv_timeout(Duration::from_secs(30))
            .expect("every rayon worker takes a holding job");
    }

    let (control_done, control) = mpsc::channel();
    let control_thread = std::thread::spawn(move || {
        let sum: u64 = (0..4096_u64).into_par_iter().sum();
        let _ = control_done.send(sum);
    });
    let control_blocked = control.recv_timeout(Duration::from_millis(500)).is_err();

    let (backend_done, backend) = mpsc::channel();
    let backend_thread = std::thread::spawn(move || {
        let topologies = topologies();
        let cases = cases(1_024, 99);
        let jobs = jobs(&topologies, &cases);
        let mut outputs = Vec::new();
        let result = CpuBackend::new(lanes(4))
            .evaluate(&jobs, &mut outputs)
            .map(|()| outputs.iter().filter(|output| output.is_some()).count());
        let _ = backend_done.send(result);
    });
    let finished = backend.recv_timeout(Duration::from_secs(60));

    release.wait();
    let control_sum = control.recv_timeout(Duration::from_secs(60));
    let _ = control_thread.join();
    let _ = backend_thread.join();

    assert!(
        control_blocked,
        "the control finished while every rayon worker was held: the pool was not saturated"
    );
    assert_eq!(control_sum.ok(), Some(4096 * 4095 / 2));
    let evaluated = finished
        .expect("the backend finished while every rayon worker was held")
        .expect("the backend evaluates");
    assert!(evaluated > 800, "{evaluated} of 1024 evaluated");
}

/// FL-043: retiring every live occupant of a packed CPU shard reclaims the
/// shard immediately, lowering `shards()` and `resident_bytes()` and returning
/// budget room for subsequent shards. Wrong generations never evict a live
/// occupant, and surviving shards stay packed without repacking.
#[test]
fn empty_shards_are_reclaimed_when_slots_retire() -> Result<(), CoreError> {
    let topologies = topologies();
    // 8 slots per topology, every case Typical (never deferred), 4 slots per shard -> 2 shards/topology.
    let mut draws = Draws(17);
    let mut cases = Vec::new();
    for (topology, shape) in topologies.iter().enumerate() {
        for _ in 0..8 {
            cases.push(Case {
                topology,
                regime: Regime::Typical,
                parameters: parameters(shape, Regime::Typical, &mut draws),
                inputs: inputs(Regime::Typical, &mut draws),
            });
        }
    }
    let topo_count = topologies.len();
    let jobs = jobs(&topologies, &cases);
    let mut backend = CpuBackend::with_shard_slots(lanes(2), 4);
    let mut outputs = Vec::new();
    backend.evaluate(&jobs, &mut outputs)?;
    let first = bits(&outputs);
    assert_eq!(backend.shards(), topo_count * 2);
    assert_eq!(backend.resident(), topo_count * 8);
    let peak_bytes = backend.resident_bytes();
    let initial_packs = backend.packs();

    // Releasing every slot with the wrong generation must not drop anything.
    let wrong_gen: Vec<ArenaSlot> = jobs
        .iter()
        .map(|job| {
            ArenaSlot::new(
                job.slot.topology(),
                job.slot.slot(),
                job.slot.generation() + 1,
            )
        })
        .collect();
    backend.release_slots(&wrong_gen);
    assert_eq!(backend.shards(), topo_count * 2);
    assert_eq!(backend.resident(), topo_count * 8);

    // Retire shard 1 (slot >= 4) of every topology, keeping shard 0 alive.
    let shard1_slots: Vec<ArenaSlot> = jobs
        .iter()
        .filter(|job| job.slot.slot() >= 4)
        .map(|job| job.slot)
        .collect();
    assert_eq!(shard1_slots.len(), topo_count * 4);
    backend.release_slots(&shard1_slots);
    assert_eq!(
        backend.shards(),
        topo_count,
        "all emptied second shards reclaimed"
    );
    assert_eq!(backend.resident(), topo_count * 4);
    assert!(
        backend.resident_bytes() < peak_bytes,
        "reclaimed shards must reduce resident_bytes ({} vs {peak_bytes})",
        backend.resident_bytes()
    );

    // Surviving shard-0 jobs evaluate bit-identically with zero repacking.
    let shard0_jobs: Vec<ControllerJob<'_>> = jobs
        .iter()
        .copied()
        .filter(|job| job.slot.slot() < 4)
        .collect();
    backend.evaluate(&shard0_jobs, &mut outputs)?;
    let expected0: Vec<_> = jobs
        .iter()
        .zip(&first)
        .filter(|(job, _)| job.slot.slot() < 4)
        .map(|(_, out)| out.clone())
        .collect();
    assert_eq!(bits(&outputs), expected0);
    assert_eq!(backend.packs(), initial_packs);

    // Under a tight budget that fits only shard 0 of each topology, retiring
    // them lets shard 1 fit instead of deferring forever.
    let tight_budget = flife_neural::BackendBudget {
        bytes: backend.resident_bytes() + 4096,
        organisms: (topo_count * 8) as u64,
    };
    let mut bounded = CpuBackend::with_budget(lanes(1), 4, tight_budget);
    bounded.evaluate(&shard0_jobs, &mut outputs)?;
    assert_eq!(bounded.shards(), topo_count);
    let shard1_jobs: Vec<ControllerJob<'_>> = jobs
        .iter()
        .copied()
        .filter(|job| job.slot.slot() >= 4)
        .collect();
    bounded.evaluate(&shard1_jobs, &mut outputs)?;
    assert!(
        outputs.iter().any(Option::is_none),
        "tight budget must defer shard 1 while shard 0 is live"
    );
    let shard0_slots: Vec<ArenaSlot> = shard0_jobs.iter().map(|job| job.slot).collect();
    bounded.release_slots(&shard0_slots);
    assert_eq!(bounded.shards(), 0);
    bounded.evaluate(&shard1_jobs, &mut outputs)?;
    assert_eq!(bounded.shards(), topo_count);
    assert!(
        outputs.iter().all(Option::is_some),
        "reclaimed budget must admit shard 1"
    );
    Ok(())
}

/// V2-01: variable `(input_width, raw_output_width)` topologies (including
/// Bundle A `108/44`, Bundle B `108/40`, All-21 `112/48`, odd custom `105/37`,
/// and recurrent state transitions via shared `update_recurrent_state`) match
/// `ReferenceController` bit-for-bit on `CpuBackend` in both F32 and F16.
#[test]
fn variable_width_reference_and_torch_bit_exact() -> Result<(), CoreError> {
    let mut draws = Draws(0x0002_0001);
    let widths: [(u32, u32, &[u32]); 6] = [
        (104, 36, &[8]),
        (104, 40, &[64, 32]),
        (108, 44, &[32, 16]),
        (112, 48, &[64, 32]),
        (105, 37, &[5]),
        (110, 76, &[16]),
    ];

    for (in_w, out_w, hidden) in widths {
        let topology = DenseTopology::with_io(in_w, hidden, out_w)?;
        assert_eq!(topology.input_width(), in_w as usize);
        assert_eq!(topology.raw_output_width(), out_w as usize);

        let f32_params: Vec<Vec<f32>> = (0..12)
            .map(|_| {
                let mut vals = vec![0.0_f32; topology.parameter_count()];
                for layer in topology.layers() {
                    let scale = 1.5 / (layer.inputs as f64).sqrt();
                    for value in &mut vals[layer.weights..layer.biases + layer.outputs] {
                        *value = (draws.symmetric() * scale) as f32;
                    }
                }
                vals
            })
            .collect();
        let f16_params: Vec<Vec<F16>> = f32_params
            .iter()
            .map(|vals| vals.iter().map(|&v| F16::from_f64(f64::from(v))).collect())
            .collect();
        let input_vecs: Vec<Vec<f64>> = (0..12)
            .map(|_| {
                (0..in_w as usize)
                    .map(|_| draws.symmetric() * 2.0)
                    .collect()
            })
            .collect();

        // F32 parity on 1 and 4 lanes.
        let f32_jobs: Vec<ControllerJob<'_>> = (0..12)
            .map(|i| {
                Ok(ControllerJob {
                    genome: GenomeId::new(i as u64 + 1),
                    slot: ArenaSlot::new(0, i as u64, 1),
                    controller: ReferenceController::new(&topology, &f32_params[i], BOUND)?,
                    inputs: &input_vecs[i],
                })
            })
            .collect::<Result<_, CoreError>>()?;

        let mut want_f32 = Vec::with_capacity(f32_jobs.len());
        let mut scratch = ControllerScratch::new(&topology)?;
        for job in &f32_jobs {
            want_f32.push(Some(
                job.controller
                    .evaluate_raw(job.inputs, &mut scratch)?
                    .iter()
                    .map(|v| v.to_bits())
                    .collect::<Vec<_>>(),
            ));
        }
        for lane_count in [1, 4] {
            let mut backend = CpuBackend::new(lanes(lane_count));
            let mut out = Vec::new();
            backend.evaluate(&f32_jobs, &mut out)?;
            assert_eq!(
                bits(&out),
                want_f32,
                "F32 ({in_w}, {out_w}) {hidden:?} lanes={lane_count}"
            );
        }

        // F16 parity.
        let f16_jobs: Vec<ControllerJob<'_>> = (0..12)
            .map(|i| {
                Ok(ControllerJob {
                    genome: GenomeId::new(100 + i as u64),
                    slot: ArenaSlot::new(0, i as u64, 1),
                    controller: ReferenceController::new(&topology, &f16_params[i], BOUND)?,
                    inputs: &input_vecs[i],
                })
            })
            .collect::<Result<_, CoreError>>()?;
        let mut want_f16 = Vec::with_capacity(f16_jobs.len());
        for job in &f16_jobs {
            want_f16.push(Some(
                job.controller
                    .evaluate_raw(job.inputs, &mut scratch)?
                    .iter()
                    .map(|v| v.to_bits())
                    .collect::<Vec<_>>(),
            ));
        }
        let mut backend_f16 = CpuBackend::new(lanes(2));
        let mut out_f16 = Vec::new();
        backend_f16.evaluate(&f16_jobs, &mut out_f16)?;
        assert_eq!(bits(&out_f16), want_f16, "F16 ({in_w}, {out_w}) {hidden:?}");
    }

    // Recurrent topology (106 sensors + S=2 -> [16] -> 40 actions + 2*2 tail = 108/44):
    // CpuBackend raw output + shared update_recurrent_state == ReferenceController::evaluate_recurrent_slice.
    let rec_topo = DenseTopology::with_io_recurrent(106, &[16], 40, 2)?;
    assert_eq!(rec_topo.input_width(), 108);
    assert_eq!(rec_topo.raw_output_width(), 44);
    let rec_params: Vec<f32> = (0..rec_topo.parameter_count())
        .map(|_| (draws.symmetric() * 0.25) as f32)
        .collect();
    let sensor_in: Vec<f64> = (0..106).map(|_| draws.symmetric() * 1.5).collect();
    let state = [0.35_f32, -0.625];
    let mut combined_in = sensor_in.clone();
    combined_in.extend(state.iter().map(|&h| f64::from(h)));

    let ctrl = ReferenceController::new(&rec_topo, &rec_params, BOUND)?;
    let mut scratch = ControllerScratch::new(&rec_topo)?;
    let mut ref_next = [0.0_f32; 2];
    let ref_actions =
        ctrl.evaluate_recurrent_slice(&sensor_in, &state, 2.5, &mut scratch, &mut ref_next)?;

    let job = ControllerJob {
        genome: GenomeId::new(999),
        slot: ArenaSlot::new(0, 0, 1),
        controller: ctrl,
        inputs: &combined_in,
    };
    let mut backend = CpuBackend::new(lanes(1));
    let mut out = Vec::new();
    backend.evaluate(&[job], &mut out)?;
    let got_raw = out[0].expect("recurrent job evaluated on CpuBackend");
    assert_eq!(got_raw.len(), 44);
    let mut torch_next = [0.0_f32; 2];
    flife_neural::update_recurrent_state(&state, &got_raw[40..44], 2.5, &mut torch_next)?;
    assert_eq!(
        got_raw[..40]
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        ref_actions.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    assert_eq!(torch_next.map(f32::to_bits), ref_next.map(f32::to_bits));
    Ok(())
}

/// V2-05 (`so2-motor`, 40 outputs) & V2-06 (`so2-motor` + `so2-budding`, 42 outputs):
/// FrankenTorch CPU (`CpuBackend`, 1 vs 4 lanes, F32 & F16) matches the scalar
/// reference bit-for-bit all the way through `decode_with_layout` (`MotorSteering`
/// & `BuddingSteering`).
#[test]
fn flife_backend_cpu_40_output_motor_and_42_output_budding_match_reference() -> Result<(), CoreError>
{
    use flife_core::TimeStep;
    use flife_core::binary16::F16;
    use flife_neural::{ActionDecoding, ActionLayout, decode_with_layout};

    let decoding = ActionDecoding::new(0.8, [0.25, 0.75], 0.355, 0.5, 1.0e-6)?;
    let step = TimeStep::new(1, 32)?;
    let mut draws = Draws(0x2026_0506);

    let specs = [
        (104_u32, ActionLayout::new(true, false, false, false, 0)?),
        (108_u32, ActionLayout::new(true, true, false, false, 0)?),
    ];

    for (in_w, layout) in specs {
        let out_w = layout.action_width() as u32;
        let topology = DenseTopology::with_io(in_w, &[64, 32], out_w)?;
        let count = 24_usize;
        let mut f32_params = Vec::with_capacity(count);
        let mut f16_params = Vec::with_capacity(count);
        let mut inputs = Vec::with_capacity(count);
        for _ in 0..count {
            let p32: Vec<f32> = (0..topology.parameter_count())
                .map(|_| (draws.symmetric() * 0.25) as f32)
                .collect();
            let p16: Vec<F16> = p32.iter().map(|&v| F16::from_f64(f64::from(v))).collect();
            let inp: Vec<f64> = (0..in_w as usize)
                .map(|_| draws.symmetric() * 1.5)
                .collect();
            f32_params.push(p32);
            f16_params.push(p16);
            inputs.push(inp);
        }

        let f32_jobs: Vec<ControllerJob<'_>> = (0..count)
            .map(|i| {
                Ok(ControllerJob {
                    genome: GenomeId::new(1_000 + i as u64),
                    slot: ArenaSlot::new(0, i as u64, 1),
                    controller: ReferenceController::new(&topology, &f32_params[i], BOUND)?,
                    inputs: &inputs[i],
                })
            })
            .collect::<Result<_, CoreError>>()?;
        let f16_jobs: Vec<ControllerJob<'_>> = (0..count)
            .map(|i| {
                Ok(ControllerJob {
                    genome: GenomeId::new(2_000 + i as u64),
                    slot: ArenaSlot::new(1, i as u64, 1),
                    controller: ReferenceController::new(&topology, &f16_params[i], BOUND)?,
                    inputs: &inputs[i],
                })
            })
            .collect::<Result<_, CoreError>>()?;

        let mut scratch = ControllerScratch::new(&topology)?;
        for (label, jobs) in [("F32", &f32_jobs), ("F16", &f16_jobs)] {
            let mut want_actions = Vec::with_capacity(count);
            for job in jobs {
                let raw = job.controller.evaluate_raw(job.inputs, &mut scratch)?;
                let raw64: Vec<f64> = raw.iter().map(|&v| f64::from(v)).collect();
                want_actions.push(decode_with_layout(&raw64, step, &decoding, &layout)?);
            }
            for lane_count in [1, 4] {
                let mut backend = CpuBackend::new(lanes(lane_count));
                let mut out = Vec::new();
                backend.evaluate(jobs, &mut out)?;
                assert_eq!(out.len(), count);
                for (idx, (got_opt, want)) in out.iter().zip(&want_actions).enumerate() {
                    let raw = got_opt.expect("CpuBackend evaluated job");
                    assert_eq!(raw.len(), out_w as usize);
                    let raw64: Vec<f64> = raw.iter().map(|&v| f64::from(v)).collect();
                    let got = decode_with_layout(&raw64, step, &decoding, &layout)?;
                    assert_eq!(
                        got, *want,
                        "{label} ({in_w}, {out_w}) lanes={lane_count} job={idx}"
                    );
                    assert!(got.motor.is_some());
                    assert_eq!(got.budding.is_some(), layout.budding_slots().is_some());
                }
            }
        }
    }
    Ok(())
}

/// V2-12 (`epigenetic-recurrence`): 1,024 organisms across all 5 catalog hidden
/// shapes for BOTH `S = 2` (`108/44`, Bundle A) and `S = 4` (`108/44`, FL-054),
/// plus unaligned `(107/41, [5])` and max-state `S = 16` (`120/68`), match
/// `ReferenceController::evaluate_recurrent_slice` bit-for-bit on `CpuBackend`
/// (F32 & F16, 1 and 4 lanes) through shared `update_recurrent_state` for both
/// `b_epi = 2.5` (V2-12 slow epigenetic bias) and `b_epi = 0.0` (v1 bit-exact).
#[test]
fn recurrent_reference_torch_f16_f32_bit_exact() -> Result<(), CoreError> {
    let mut draws = Draws(0x2026_0012);

    struct Suite<'a> {
        label: &'a str,
        sensor_w: u32,
        action_w: u32,
        state_w: u32,
        hiddens: &'a [&'a [u32]],
        count: usize,
    }

    // 1,024 organisms distributed across all 5 catalog hidden shapes for S=2 (108/44)
    // and S=4 (108/44).
    let suites = [
        Suite {
            label: "S=2 108/44 (Bundle A)",
            sensor_w: 106,
            action_w: 40,
            state_w: 2,
            hiddens: &TOPOLOGIES,
            count: 1_024,
        },
        Suite {
            label: "S=4 108/44 (FL-054)",
            sensor_w: 104,
            action_w: 36,
            state_w: 4,
            hiddens: &TOPOLOGIES,
            count: 1_024,
        },
        Suite {
            label: "S=2 107/41 odd",
            sensor_w: 105,
            action_w: 37,
            state_w: 2,
            hiddens: &[&[5], &[8]],
            count: 32,
        },
        Suite {
            label: "S=16 120/68 max-state",
            sensor_w: 104,
            action_w: 36,
            state_w: 16,
            hiddens: &[&[32]],
            count: 32,
        },
    ];

    for (
        suite_idx,
        &Suite {
            label,
            sensor_w,
            action_w,
            state_w,
            hiddens,
            count,
        },
    ) in suites.iter().enumerate()
    {
        let topologies: Vec<DenseTopology> = hiddens
            .iter()
            .map(|h| DenseTopology::with_io_recurrent(sensor_w, h, action_w, state_w))
            .collect::<Result<_, _>>()?;
        let s = state_w as usize;
        let a = action_w as usize;
        let raw_w = a + 2 * s;
        let in_w = sensor_w as usize + s;

        let mut f32_params = Vec::with_capacity(count);
        let mut f16_params = Vec::with_capacity(count);
        let mut sensor_vecs = Vec::with_capacity(count);
        let mut state_vecs = Vec::with_capacity(count);
        let mut combined_vecs = Vec::with_capacity(count);
        let mut topo_ids = Vec::with_capacity(count);
        for i in 0..count {
            let t = i % topologies.len();
            let topo = &topologies[t];
            assert_eq!(topo.input_width(), in_w);
            assert_eq!(topo.raw_output_width(), raw_w);
            let mut p32 = vec![0.0_f32; topo.parameter_count()];
            for layer in topo.layers() {
                let scale = 1.0 / (layer.inputs as f64).sqrt();
                for v in &mut p32[layer.weights..layer.biases + layer.outputs] {
                    *v = (draws.symmetric() * scale) as f32;
                }
            }
            let p16: Vec<F16> = p32.iter().map(|&v| F16::from_f64(f64::from(v))).collect();
            let sensors: Vec<f64> = (0..sensor_w as usize)
                .map(|_| draws.symmetric() * 1.5)
                .collect();
            let state: Vec<f32> = (0..s).map(|_| (draws.symmetric() * 0.95) as f32).collect();
            let mut combined = Vec::with_capacity(in_w);
            combined.extend_from_slice(&sensors);
            combined.extend(state.iter().map(|&h| f64::from(h)));
            f32_params.push(p32);
            f16_params.push(p16);
            sensor_vecs.push(sensors);
            state_vecs.push(state);
            combined_vecs.push(combined);
            topo_ids.push(t);
        }

        let mut next_slot = vec![0_u64; topologies.len()];
        let mut f32_jobs = Vec::with_capacity(count);
        let mut f16_jobs = Vec::with_capacity(count);
        for i in 0..count {
            let t = topo_ids[i];
            let slot = next_slot[t];
            next_slot[t] += 1;
            let bucket = (suite_idx * 8 + t) as u32;
            f32_jobs.push(ControllerJob {
                genome: GenomeId::new((suite_idx as u64 + 1) * 100_000 + i as u64),
                slot: ArenaSlot::new(bucket, slot, 1),
                controller: ReferenceController::new(&topologies[t], &f32_params[i], BOUND)?,
                inputs: &combined_vecs[i],
            });
            f16_jobs.push(ControllerJob {
                genome: GenomeId::new((suite_idx as u64 + 1) * 200_000 + i as u64),
                slot: ArenaSlot::new(bucket, slot, 1),
                controller: ReferenceController::new(&topologies[t], &f16_params[i], BOUND)?,
                inputs: &combined_vecs[i],
            });
        }

        let mut scratch = ControllerScratch::new(topologies.last().expect("topology"))?;
        for (dtype_label, jobs) in [("F32", &f32_jobs), ("F16", &f16_jobs)] {
            let mut want_actions = Vec::with_capacity(count);
            let mut want_next_epi = Vec::with_capacity(count);
            let mut want_next_v1 = Vec::with_capacity(count);
            for (i, job) in jobs.iter().enumerate() {
                let mut h_epi = vec![0.0_f32; s];
                let mut h_v1 = vec![0.0_f32; s];
                let acts_epi = job.controller.evaluate_recurrent_slice(
                    &sensor_vecs[i],
                    &state_vecs[i],
                    2.5,
                    &mut scratch,
                    &mut h_epi,
                )?;
                let acts_v1 = job.controller.evaluate_recurrent_slice(
                    &sensor_vecs[i],
                    &state_vecs[i],
                    0.0,
                    &mut scratch,
                    &mut h_v1,
                )?;
                assert_eq!(
                    acts_epi.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    acts_v1.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                );
                want_actions.push(acts_epi);
                want_next_epi.push(h_epi);
                want_next_v1.push(h_v1);
            }

            for lane_count in [1, 4] {
                let mut backend = CpuBackend::new(lanes(lane_count));
                let mut out = Vec::new();
                backend.evaluate(jobs, &mut out)?;
                assert_eq!(out.len(), count);
                assert!(
                    out.iter().all(Option::is_some),
                    "{label} {dtype_label} lanes={lane_count}: unexpected deferral"
                );
                let mut got_epi = vec![0.0_f32; s];
                let mut got_v1 = vec![0.0_f32; s];
                for (i, got_opt) in out.iter().enumerate() {
                    let raw = got_opt.expect("evaluated");
                    assert_eq!(raw.len(), raw_w);
                    assert_eq!(
                        raw[..a].iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                        want_actions[i]
                            .iter()
                            .map(|v| v.to_bits())
                            .collect::<Vec<_>>(),
                        "{label} {dtype_label} lanes={lane_count} org={i} actions"
                    );
                    flife_neural::update_recurrent_state(
                        &state_vecs[i],
                        &raw[a..raw_w],
                        2.5,
                        &mut got_epi,
                    )?;
                    flife_neural::update_recurrent_state(
                        &state_vecs[i],
                        &raw[a..raw_w],
                        0.0,
                        &mut got_v1,
                    )?;
                    assert_eq!(
                        got_epi.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                        want_next_epi[i]
                            .iter()
                            .map(|v| v.to_bits())
                            .collect::<Vec<_>>(),
                        "{label} {dtype_label} lanes={lane_count} org={i} h_next(b_epi=2.5)"
                    );
                    assert_eq!(
                        got_v1.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                        want_next_v1[i]
                            .iter()
                            .map(|v| v.to_bits())
                            .collect::<Vec<_>>(),
                        "{label} {dtype_label} lanes={lane_count} org={i} h_next(b_epi=0.0)"
                    );
                }
            }
        }
    }
    Ok(())
}

/// Regression for FL-043 budget adherence when widening hidden layers across
/// topologies (`[64, 32]` -> `[96, 64]`): `Scratch::hold` must reserve exact
/// activation capacity so `resident_bytes()` never overshoots `budget.bytes`
/// via exponential `Vec` doubling, and `release_slots` reclaims empty shards.
#[test]
fn cpu_backend_budget_holds_strictly_when_widening_hidden_layers_and_across_release_slots()
-> Result<(), CoreError> {
    let narrow = DenseTopology::new(&[64, 32])?;
    let wide = DenseTopology::new(&[96, 64])?;
    let narrow_params = vec![0.0_f32; narrow.parameter_count()];
    let wide_params = vec![0.0_f32; wide.parameter_count()];
    let inputs = [0.0_f64; SENSOR_WIDTH];

    let shard_slots = 4_u64;
    let occupant_bytes = size_of::<Option<(u32, GenomeId)>>() as u64;
    let shard_bytes_for =
        |params: usize| (params as u64) * shard_slots * 4 + shard_slots * occupant_bytes;
    let lane_count = 2_usize;
    let exact_wide_activations = (lane_count as u64) * 2 * (96 * 4);
    let tight_budget = BackendBudget {
        bytes: shard_bytes_for(narrow.parameter_count())
            + shard_bytes_for(wide.parameter_count())
            + exact_wide_activations,
        organisms: 256,
    };

    let mut backend = CpuBackend::with_budget(lanes(lane_count), shard_slots, tight_budget);
    let mut out = Vec::new();

    // Tick 1: populate `[64, 32]` across 2 lanes (widest = 64).
    let narrow_slots: Vec<ArenaSlot> = (0..4).map(|s| ArenaSlot::new(0, s, 1)).collect();
    let narrow_jobs: Vec<ControllerJob<'_>> = (0..200)
        .map(|i| {
            let slot = narrow_slots[i % 4];
            ControllerJob {
                genome: GenomeId::new(10 + (i % 4) as u64),
                slot,
                controller: ReferenceController::new(&narrow, &narrow_params, BOUND)
                    .expect("narrow controller"),
                inputs: &inputs,
            }
        })
        .collect();
    backend.evaluate(&narrow_jobs, &mut out)?;
    assert!(out.iter().all(Option::is_some));
    assert_eq!(backend.shards(), 1);
    assert!(
        backend.resident_bytes() <= tight_budget.bytes,
        "after narrow tick: resident {} > budget {}",
        backend.resident_bytes(),
        tight_budget.bytes
    );

    // Tick 2: introduce `[96, 64]` (widest = 96). Previously `try_reserve`
    // doubled capacity 64 -> 128 per activation buffer and exceeded `tight_budget.bytes`.
    let wide_slots: Vec<ArenaSlot> = (0..4).map(|s| ArenaSlot::new(1, s, 1)).collect();
    let wide_jobs: Vec<ControllerJob<'_>> = (0..200)
        .map(|i| {
            let slot = wide_slots[i % 4];
            ControllerJob {
                genome: GenomeId::new(100 + (i % 4) as u64),
                slot,
                controller: ReferenceController::new(&wide, &wide_params, BOUND)
                    .expect("wide controller"),
                inputs: &inputs,
            }
        })
        .collect();
    backend.evaluate(&wide_jobs, &mut out)?;
    assert!(out.iter().all(Option::is_some));
    assert_eq!(backend.shards(), 2);
    assert!(
        backend.resident_bytes() <= tight_budget.bytes,
        "after widening 64 -> 96: resident {} > budget {}",
        backend.resident_bytes(),
        tight_budget.bytes
    );

    // Releasing all narrow slots reclaims the narrow shard and drops resident bytes.
    let before_release = backend.resident_bytes();
    backend.release_slots(&narrow_slots);
    assert_eq!(backend.shards(), 1);
    assert_eq!(
        before_release - backend.resident_bytes(),
        shard_bytes_for(narrow.parameter_count())
    );
    backend.release_slots(&wide_slots);
    assert_eq!(backend.shards(), 0);
    assert_eq!(backend.resident_bytes(), exact_wide_activations);
    Ok(())
}

#[expect(clippy::panic, reason = "test helper: a mismatch is a test failure")]
fn assert_flat_matches_legacy(
    label: &str,
    legacy_out: &[Option<RawOutputs>],
    flat_out: &FlatOutputs,
) {
    assert_eq!(flat_out.valid.len(), legacy_out.len(), "{label}: valid len");
    for (i, want) in legacy_out.iter().enumerate() {
        match (want, flat_out.row(i)) {
            (Some(w), Some(g)) => assert_eq!(
                g.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                w.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                "{label} row {i} differs"
            ),
            (None, None) => {}
            (w, g) => panic!("{label} row {i} deferred mismatch: {w:?} vs {g:?}"),
        }
    }
}

/// Round-2 flat-F32 fast path (`ControllerBackend::evaluate_flat_f32`) must
/// match `CpuBackend::evaluate` bit-for-bit across all 5 interleaved catalog
/// topologies, all numerical regimes (including subnormals and overflows),
/// F32 and F16 weights, single-lane and multi-lane persistent-pool execution,
/// non-finite input flags, shrink/empty reuse, and error cleanup.
#[test]
fn cpu_backend_flat_f32_matches_evaluate_bit_for_bit_across_lanes_and_dtypes()
-> Result<(), CoreError> {
    let catalog = topologies();
    let batch_cases_f32 = cases(640, 2026);
    let legacy_f32 = jobs(&catalog, &batch_cases_f32);
    let batch_cases_f16: Vec<Case> = cases(640, 2027)
        .into_iter()
        .filter(|case| !matches!(case.regime, Regime::HiddenOverflow | Regime::OutputOverflow))
        .collect();
    let f16_params: Vec<Vec<F16>> = batch_cases_f16
        .iter()
        .map(|case| {
            case.parameters
                .iter()
                .map(|&v| F16::from_f64(f64::from(v)))
                .collect()
        })
        .collect();
    let legacy_f16_base = jobs(&catalog, &batch_cases_f16);
    let legacy_f16: Vec<ControllerJob<'_>> = legacy_f16_base
        .iter()
        .zip(&batch_cases_f16)
        .zip(&f16_params)
        .map(|((job, case), weights)| ControllerJob {
            genome: job.genome,
            slot: job.slot,
            controller: ReferenceController::new(
                &catalog[case.topology],
                weights.as_slice(),
                BOUND,
            )
            .expect("admitted f16 controller"),
            inputs: job.inputs,
        })
        .collect();

    let encode_inputs = |batch: &[Case]| -> (Vec<f32>, Vec<u8>) {
        let mut inputs_f32 = Vec::with_capacity(batch.len() * SENSOR_WIDTH);
        let mut inputs_finite = Vec::with_capacity(batch.len());
        for case in batch {
            let mut all_finite = true;
            for &v in &case.inputs {
                let narrowed = v as f32;
                all_finite &= narrowed.is_finite();
                inputs_f32.push(narrowed);
            }
            inputs_finite.push(u8::from(all_finite));
        }
        (inputs_f32, inputs_finite)
    };
    let (inputs_f32_for_f32, inputs_finite_for_f32) = encode_inputs(&batch_cases_f32);
    let (inputs_f32_for_f16, inputs_finite_for_f16) = encode_inputs(&batch_cases_f16);

    for (label, legacy, inputs_f32, inputs_finite) in [
        (
            "F32",
            &legacy_f32,
            &inputs_f32_for_f32,
            &inputs_finite_for_f32,
        ),
        (
            "F16",
            &legacy_f16,
            &inputs_f32_for_f16,
            &inputs_finite_for_f16,
        ),
    ] {
        let flat_jobs: Vec<FlatControllerJob<'_>> = legacy
            .iter()
            .map(|job| FlatControllerJob {
                genome: job.genome,
                slot: job.slot,
                controller: job.controller,
            })
            .collect();
        let mut legacy_backend = CpuBackend::new(lanes(1));
        let mut legacy_out = Vec::new();
        legacy_backend.evaluate(legacy, &mut legacy_out)?;

        for lane_count in [1_usize, 4] {
            let mut backend = CpuBackend::with_shard_slots(lanes(lane_count), 32);
            assert!(backend.supports_flat_f32());
            let mut flat_out = FlatOutputs::default();
            backend.evaluate_flat_f32(
                &flat_jobs,
                SENSOR_WIDTH,
                inputs_f32,
                inputs_finite,
                &mut flat_out,
            )?;
            let packs_after_first = backend.packs();
            assert_eq!(flat_out.output_width, ACTION_WIDTH);
            assert_flat_matches_legacy(
                &format!("{label} lanes={lane_count}"),
                &legacy_out,
                &flat_out,
            );

            // Steady-state second tick must reuse packed shards with 0 repacks.
            backend.evaluate_flat_f32(
                &flat_jobs,
                SENSOR_WIDTH,
                inputs_f32,
                inputs_finite,
                &mut flat_out,
            )?;
            assert_eq!(backend.packs(), packs_after_first);

            // Shrink, empty, and malformed-length error cleanup.
            let half = flat_jobs.len() / 2;
            backend.evaluate_flat_f32(
                &flat_jobs[..half],
                SENSOR_WIDTH,
                &inputs_f32[..half * SENSOR_WIDTH],
                &inputs_finite[..half],
                &mut flat_out,
            )?;
            assert_flat_matches_legacy(
                &format!("{label} shrink lanes={lane_count}"),
                &legacy_out[..half],
                &flat_out,
            );

            backend.evaluate_flat_f32(&[], SENSOR_WIDTH, &[], &[], &mut flat_out)?;
            assert!(flat_out.valid.is_empty() && flat_out.values.is_empty());
            assert_eq!(flat_out.output_width, 0);

            backend.evaluate_flat_f32(
                &flat_jobs[..8],
                SENSOR_WIDTH,
                &inputs_f32[..8 * SENSOR_WIDTH],
                &inputs_finite[..8],
                &mut flat_out,
            )?;
            let err = backend.evaluate_flat_f32(
                &flat_jobs[..8],
                SENSOR_WIDTH,
                &inputs_f32[..4 * SENSOR_WIDTH],
                &inputs_finite[..8],
                &mut flat_out,
            );
            assert!(matches!(err, Err(CoreError::LayoutMismatch { .. })));
            assert!(flat_out.valid.is_empty() && flat_out.values.is_empty());
            assert_eq!(flat_out.output_width, 0);
        }
    }
    Ok(())
}

/// Round-2b bit-exact parity for `tanh_f32_4` + scalar tail activation, 8x8
/// register-tiled transpose (`pack_as`), and parallel bulk shard resize+pack
/// (`>= 64` newly-admitted jobs on multi-lane `CpuBackend`):
/// - covers all 5 catalog topologies plus an odd non-multiple-of-4 hidden
///   topology `[7, 5]` (exercising both `tanh_f32_4` and scalar `tanh_f32`
///   tails as well as interior 8x8 tiles and odd remainder strips);
/// - covers F32 and F16 weights, multi-shard parallel resize (`shard_slots=16`),
///   far sparse shard IDs (`1 << 40`), same-batch duplicate-slot overwrite
///   (last writer wins, earlier deferred), steady-state zero-repack tick 2, and
///   cold `evaluate_flat_f32` bulk pack.
#[test]
fn cpu_backend_parallel_bulk_pack_and_tanh_f32_4_match_serial_and_reference_bit_for_bit()
-> Result<(), CoreError> {
    let mut catalog = topologies();
    catalog.push(DenseTopology::with_io(
        SENSOR_WIDTH as u32,
        &[7, 5],
        ACTION_WIDTH as u32,
    )?);
    let odd_topo_idx = catalog.len() - 1;
    let mut draws = Draws(0x2026_020B);
    let mut custom_cases = Vec::with_capacity(320);
    for idx in 0..320_usize {
        let topology = if idx % 6 == 0 {
            odd_topo_idx
        } else {
            idx % (catalog.len() - 1)
        };
        let regime = match idx % 16 {
            0..=9 => Regime::Typical,
            10..=12 => Regime::Saturating,
            13 | 14 => Regime::Tiny,
            _ => Regime::WideInput,
        };
        custom_cases.push(Case {
            topology,
            regime,
            parameters: parameters(&catalog[topology], regime, &mut draws),
            inputs: inputs(regime, &mut draws),
        });
    }
    let f16_params: Vec<Vec<F16>> = custom_cases
        .iter()
        .map(|case| {
            case.parameters
                .iter()
                .map(|&v| F16::from_f64(f64::from(v)))
                .collect()
        })
        .collect();

    for (label, first_slot, shard_slots, as_f16) in [
        ("F32 dense multi-shard", 0_u64, 16_u64, false),
        ("F16 dense multi-shard", 0_u64, 16_u64, true),
        ("F32 sparse 1<<40", 1_u64 << 40, 32_u64, false),
    ] {
        let mut next = vec![first_slot; catalog.len()];
        let mut batch = Vec::with_capacity(custom_cases.len() + 1);
        for (idx, case) in custom_cases.iter().enumerate() {
            let slot = next[case.topology];
            next[case.topology] += 1;
            let controller = if as_f16 {
                ReferenceController::new(&catalog[case.topology], &f16_params[idx], BOUND)?
            } else {
                ReferenceController::new(&catalog[case.topology], &case.parameters, BOUND)?
            };
            batch.push(ControllerJob {
                genome: GenomeId::new(idx as u64 + 1),
                slot: ArenaSlot::new(case.topology as u32, slot, 1),
                controller,
                inputs: &case.inputs,
            });
        }
        // Append a same-batch duplicate-slot overwrite (higher generation on
        // slot 0 of the odd `[7, 5]` topology) inside the `>= 64` bulk batch.
        let dup_target = batch[0];
        let dup_source = batch
            .iter()
            .find(|job| {
                job.slot.topology() == dup_target.slot.topology() && job.slot != dup_target.slot
            })
            .copied()
            .expect("second odd-topology job");
        batch.push(ControllerJob {
            genome: GenomeId::new(999_001),
            slot: ArenaSlot::new(
                dup_target.slot.topology(),
                dup_target.slot.slot(),
                dup_target.slot.generation() + 1,
            ),
            controller: dup_source.controller,
            inputs: dup_target.inputs,
        });
        let mut serial = CpuBackend::with_shard_slots(lanes(1), shard_slots);
        let mut serial_out = Vec::new();
        serial.evaluate(&batch, &mut serial_out)?;

        // Earlier generation of overwritten slot 0 must be deferred; later
        // generation at the tail must evaluate and match the reference.
        assert_eq!(serial_out[0], None, "{label}: overwritten slot 0 deferred");
        assert!(
            serial_out.last().and_then(Option::as_ref).is_some(),
            "{label}: overwriting tail job evaluated"
        );

        let mut parallel = CpuBackend::with_shard_slots(lanes(4), shard_slots);
        let mut parallel_out = Vec::new();
        parallel.evaluate(&batch, &mut parallel_out)?;
        assert_eq!(
            bits(&parallel_out),
            bits(&serial_out),
            "{label}: 4-lane vs 1-lane"
        );
        assert_eq!(
            parallel.resident(),
            serial.resident(),
            "{label}: resident count matches"
        );
        assert_eq!(
            parallel.shards(),
            serial.shards(),
            "{label}: shard count matches"
        );
        assert_eq!(
            parallel.packs(),
            custom_cases.len() as u64,
            "{label}: deduplicated unique slot pack count"
        );

        // Steady-state second tick over the surviving unique slots packs 0.
        let steady = &batch[1..];
        let mut steady_out = Vec::new();
        parallel.evaluate(steady, &mut steady_out)?;
        assert_eq!(
            parallel.packs(),
            custom_cases.len() as u64,
            "{label}: 0 repacks on steady-state tick"
        );
        assert_eq!(bits(&steady_out), bits(&serial_out)[1..].to_vec());

        // Verify cold bulk parallel pack on `evaluate_flat_f32` matches bit-for-bit.
        let mut inputs_f32 = Vec::with_capacity(batch.len() * SENSOR_WIDTH);
        let mut inputs_finite = Vec::with_capacity(batch.len());
        for job in &batch {
            let mut all_finite = true;
            for &v in job.inputs {
                let narrowed = v as f32;
                all_finite &= narrowed.is_finite();
                inputs_f32.push(narrowed);
            }
            inputs_finite.push(u8::from(all_finite));
        }
        let flat_jobs: Vec<FlatControllerJob<'_>> = batch
            .iter()
            .map(|job| FlatControllerJob {
                genome: job.genome,
                slot: job.slot,
                controller: job.controller,
            })
            .collect();
        let mut flat_parallel = CpuBackend::with_shard_slots(lanes(4), shard_slots);
        let mut flat_out = FlatOutputs::default();
        flat_parallel.evaluate_flat_f32(
            &flat_jobs,
            SENSOR_WIDTH,
            &inputs_f32,
            &inputs_finite,
            &mut flat_out,
        )?;
        assert_flat_matches_legacy(&format!("{label} flat"), &serial_out, &flat_out);

        // Cross-check non-deferred odd-topology `[7, 5]` rows directly against ReferenceController.
        let mut ref_scratch = ControllerScratch::new(&catalog[odd_topo_idx])?;
        let mut checked_odd = 0_usize;
        for (job, got) in batch.iter().zip(&parallel_out) {
            if job.slot.topology() == odd_topo_idx as u32
                && let Some(got_raw) = got
            {
                let want = job.controller.evaluate_raw(job.inputs, &mut ref_scratch)?;
                assert_eq!(
                    got_raw.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    want.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    "{label}: odd [7, 5] bit-exact vs ReferenceController"
                );
                checked_odd += 1;
            }
        }
        assert!(checked_odd >= 40, "{label}: checked {checked_odd} odd rows");
    }
    Ok(())
}
