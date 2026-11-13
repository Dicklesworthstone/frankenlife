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
    ACTION_WIDTH, ArenaSlot, ControllerBackend, ControllerJob, ControllerScratch, DenseTopology,
    ReferenceController, SENSOR_WIDTH,
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

type Outputs = Vec<Option<[f32; ACTION_WIDTH]>>;

fn bits(outputs: &Outputs) -> Vec<Option<[u32; ACTION_WIDTH]>> {
    outputs
        .iter()
        .map(|output| output.map(|raw| raw.map(f32::to_bits)))
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
                .evaluate(job.inputs, &mut scratch)
                .ok()
                .map(|raw| raw.map(f32::to_bits)),
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
        .map(|raw| raw.map(f32::to_bits));
    assert_eq!(outputs[0].map(|raw| raw.map(f32::to_bits)), want);
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
    assert_eq!(outputs[0].map(|raw| raw.map(f32::to_bits)), want);

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
