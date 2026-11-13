//! FL-026: the Metal backend keeps each genome's parameters at its arena
//! slot in its topology's device shards. A genome is uploaded once while it
//! lives, a later occupant of its slot is uploaded in its place, a job is
//! evaluated only from its own generation's and genome's parameters, and the
//! outputs are the reference's bits in shards of any size and at slots far
//! past 32 bits.

use flife_backend_metal::{BackendBudget, MetalBackend, SHARD_BYTES};
use flife_core::CoreError;
use flife_core::binary16::F16;
use flife_core::ids::GenomeId;
use flife_neural::{
    ACTION_WIDTH, ArenaSlot, ControllerBackend, ControllerJob, ControllerScratch, DenseTopology,
    ReferenceController, SENSOR_WIDTH,
};

/// The `dense-104-36-v1` catalog's hidden widths.
const TOPOLOGIES: [&[u32]; 5] = [&[8], &[32], &[64, 32], &[96, 64], &[128, 96]];

/// Fan-in-scaled parameters admit far below this.
const BOUND: f64 = 4.0;

#[expect(clippy::panic, reason = "test helper: a failure is a test failure")]
fn backend(shard_slots: u64) -> Option<MetalBackend> {
    match MetalBackend::with_shard_slots(shard_slots) {
        Ok(backend) => Some(backend),
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => None,
        Err(error) => panic!("this host should have Metal: {error}"),
    }
}

/// SplitMix64, and uniform draws in `[-1, 1)` from it.
struct Draws(u64);

impl Draws {
    fn symmetric(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1_u64 << 52) as f64 - 1.0
    }
}

struct Case {
    topology: usize,
    parameters: Vec<f32>,
    inputs: [f64; SENSOR_WIDTH],
}

#[expect(clippy::expect_used, reason = "test helper")]
fn topologies() -> Vec<DenseTopology> {
    TOPOLOGIES
        .iter()
        .map(|widths| DenseTopology::new(widths))
        .collect::<Result<_, _>>()
        .expect("the catalog's topologies")
}

/// `count` organisms over every topology, fan-in-scaled weights and
/// observation-sized inputs.
fn cases(topologies: &[DenseTopology], count: usize) -> Vec<Case> {
    let mut draws = Draws(2026);
    (0..count)
        .map(|index| {
            let topology = index % topologies.len();
            let mut parameters = vec![0.0_f32; topologies[topology].parameter_count()];
            for layer in topologies[topology].layers() {
                let scale = 1.0 / (layer.inputs as f64).sqrt();
                for value in &mut parameters[layer.weights..layer.biases + layer.outputs] {
                    *value = (draws.symmetric() * scale) as f32;
                }
            }
            let mut inputs = [0.0; SENSOR_WIDTH];
            for value in &mut inputs {
                *value = draws.symmetric() * 2.0;
            }
            Case {
                topology,
                parameters,
                inputs,
            }
        })
        .collect()
}

/// One job per case, each topology's slots numbered from `first`.
#[expect(clippy::expect_used, reason = "test helper")]
fn jobs<'a>(
    topologies: &'a [DenseTopology],
    cases: &'a [Case],
    first: u64,
) -> Vec<ControllerJob<'a>> {
    let mut next = [first; TOPOLOGIES.len()];
    cases
        .iter()
        .enumerate()
        .map(|(index, case)| {
            let slot = next[case.topology];
            next[case.topology] += 1;
            ControllerJob {
                genome: GenomeId::new(index as u64 + 1),
                slot: ArenaSlot::new(case.topology as u32, slot, 1),
                controller: ReferenceController::new(
                    &topologies[case.topology],
                    &case.parameters,
                    BOUND,
                )
                .expect("an admitted controller"),
                inputs: &case.inputs,
            }
        })
        .collect()
}

type Bits = Vec<Option<[u32; ACTION_WIDTH]>>;

fn bits(outputs: &[Option<[f32; ACTION_WIDTH]>]) -> Bits {
    outputs
        .iter()
        .map(|output| output.map(|raw| raw.map(f32::to_bits)))
        .collect()
}

/// The reference's outputs for each job.
#[expect(clippy::expect_used, reason = "test helper")]
fn reference(jobs: &[ControllerJob<'_>]) -> Bits {
    jobs.iter()
        .map(|job| {
            let mut scratch = ControllerScratch::new(job.controller.topology()).expect("scratch");
            job.controller
                .evaluate(job.inputs, &mut scratch)
                .ok()
                .map(|raw| raw.map(f32::to_bits))
        })
        .collect()
}

#[test]
fn a_genome_is_uploaded_once_and_a_later_occupant_takes_its_place() -> Result<(), CoreError> {
    let Some(mut backend) = backend(16) else {
        return Ok(());
    };
    let topologies = topologies();
    let cases = cases(&topologies, 300);
    let jobs = jobs(&topologies, &cases, 0);
    let want = reference(&jobs);
    let mut outputs = Vec::new();
    backend.evaluate(&jobs, &mut outputs)?;
    let first = bits(&outputs);
    // Deferred organisms are the reference's to evaluate; the rest are its
    // bits, and nearly all were evaluated on the device.
    let served = first.iter().filter(|output| output.is_some()).count();
    assert!(served > 290, "{served} of 300 served");
    for (got, want) in first.iter().zip(&want) {
        if got.is_some() {
            assert_eq!(got, want);
        }
    }
    // 60 genomes of each topology, 16 to a shard: four shards each.
    assert_eq!((backend.uploads(), backend.resident_genomes()), (300, 300));
    assert_eq!(backend.shards(), 5 * 4);
    // FL-030: the command ran one kernel per shard, and the device timed it.
    let command = backend.last_command();
    assert!(
        command.is_some_and(|(kernels, seconds)| kernels == 20 && seconds > 0.0),
        "{command:?}"
    );

    // Batches of a few upload nothing and drop nothing.
    backend.evaluate(&jobs[..100], &mut outputs)?;
    assert_eq!(bits(&outputs), first[..100].to_vec());
    backend.evaluate(&jobs[200..], &mut outputs)?;
    assert_eq!(bits(&outputs), first[200..].to_vec());
    backend.evaluate(&jobs, &mut outputs)?;
    assert_eq!(bits(&outputs), first);
    assert_eq!((backend.uploads(), backend.resident_genomes()), (300, 300));

    // Genome 1's slot at the next generation: another genome with other
    // values, uploaded in its place, with its own reference's outputs.
    let moved: Vec<f32> = cases[0].parameters.iter().map(|value| -value).collect();
    let mut changed = jobs.clone();
    changed[0].genome = GenomeId::new(1_000);
    changed[0].slot = ArenaSlot::new(jobs[0].slot.topology(), jobs[0].slot.slot(), 2);
    changed[0].controller =
        ReferenceController::new(&topologies[cases[0].topology], &moved, BOUND)?;
    backend.evaluate(&changed, &mut outputs)?;
    let renewed = reference(&changed[..1]);
    assert_eq!(bits(&outputs)[0], renewed[0]);
    assert_ne!(bits(&outputs)[0], first[0]);
    assert_eq!(bits(&outputs)[1..], first[1..]);
    assert_eq!((backend.uploads(), backend.resident_genomes()), (301, 300));

    // One batch naming the slot at both generations: the slot holds the last
    // one uploaded, and the other is deferred, never given its outputs.
    let both = [changed[0], jobs[0]];
    backend.evaluate(&both, &mut outputs)?;
    assert_eq!(bits(&outputs), [None, first[0]]);

    // The same slot and generation with another genome is another occupant.
    let mut renamed = changed[0];
    renamed.slot = jobs[0].slot;
    backend.evaluate(&[renamed], &mut outputs)?;
    assert_eq!(bits(&outputs), renewed);

    // A job naming a bucket made for another topology is deferred.
    let mut misfiled = jobs[1];
    misfiled.slot = ArenaSlot::new(jobs[0].slot.topology(), 999, 1);
    backend.evaluate(&[misfiled], &mut outputs)?;
    assert_eq!(outputs, [None]);

    // The owner holds each shard's parameters and job words, and the
    // inputs, outputs and fault flags, and nothing else.
    let usage = backend.owner().usage();
    assert_eq!(usage.buffers, 2 * backend.shards() + 3, "{usage:?}");

    backend.forget();
    assert_eq!(
        (
            backend.uploads(),
            backend.resident_genomes(),
            backend.shards()
        ),
        (0, 0, 0)
    );
    backend.evaluate(&[], &mut outputs)?;
    assert!(outputs.is_empty());
    assert_eq!(
        backend.last_command(),
        None,
        "a batch of nothing ran no command"
    );
    Ok(())
}

/// Each case's parameters rounded once to binary16.
fn halves(cases: &[Case]) -> Vec<Vec<F16>> {
    cases
        .iter()
        .map(|case| {
            case.parameters
                .iter()
                .map(|&value| F16::from_f64(f64::from(value)))
                .collect()
        })
        .collect()
}

/// [`jobs`] from `first` with each case's binary16 parameters.
fn half_jobs<'a>(
    topologies: &'a [DenseTopology],
    cases: &'a [Case],
    halves: &'a [Vec<F16>],
) -> Result<Vec<ControllerJob<'a>>, CoreError> {
    let mut next = [0_u64; TOPOLOGIES.len()];
    let mut jobs = Vec::with_capacity(cases.len());
    for (index, (case, half)) in cases.iter().zip(halves).enumerate() {
        let slot = next[case.topology];
        next[case.topology] += 1;
        jobs.push(ControllerJob {
            genome: GenomeId::new(index as u64 + 1),
            slot: ArenaSlot::new(case.topology as u32, slot, 1),
            controller: ReferenceController::new(&topologies[case.topology], half, BOUND)?,
            inputs: &case.inputs,
        });
    }
    Ok(jobs)
}

/// FL-030: every catalog topology with its parameters rounded once to
/// binary16 (compact F16), binary16 subnormals included. The backend keeps
/// each genome as binary16 in its shard and the half kernel widens each
/// value exactly, so the device's outputs are the reference's bit for bit;
/// nothing is deferred.
#[test]
fn every_topology_in_f16_is_the_reference_bit_for_bit() -> Result<(), CoreError> {
    let Some(mut backend) = backend(64) else {
        return Ok(());
    };
    let topologies = topologies();
    let cases = cases(&topologies, 500);
    let halves = halves(&cases);
    let subnormal = |half: &F16| half.to_bits() & 0x7C00 == 0 && half.to_bits() & 0x03FF != 0;
    assert!(
        halves.iter().flatten().any(subnormal),
        "no binary16 subnormal"
    );
    let jobs = half_jobs(&topologies, &cases, &halves)?;
    let mut outputs = Vec::new();
    backend.evaluate(&jobs, &mut outputs)?;
    let want = reference(&jobs);
    assert!(
        want.iter().all(Option::is_some),
        "the reference refused one"
    );
    assert_eq!(bits(&outputs), want);
    assert_eq!(backend.uploads(), jobs.len() as u64);

    // An F32 genome at a slot of a member whose shards hold binary16 is
    // deferred, never read as binary16.
    let wide = self::jobs(&topologies, &cases[..1], 0);
    backend.evaluate(&wide, &mut outputs)?;
    assert_eq!(outputs, [None]);
    assert_eq!(backend.uploads(), jobs.len() as u64);
    Ok(())
}

/// FL-030: a binary16 shard holds two bytes a parameter. The same slots in
/// F32 take exactly two bytes a parameter more on the device, and nothing
/// else differs; and under a budget of two thirds of what the F32 backend
/// holds, some F32 jobs are deferred while every F16 job is uploaded and is
/// the reference's bits, since admission charges a shard at its dtype's
/// size.
#[test]
fn an_f16_shard_holds_half_the_bytes_and_is_admitted_at_that_size() -> Result<(), CoreError> {
    let (Some(mut wide), Some(mut half)) = (backend(64), backend(64)) else {
        return Ok(());
    };
    let topologies = topologies();
    let cases = cases(&topologies, 500);
    let halves = halves(&cases);
    let wide_jobs = jobs(&topologies, &cases, 0);
    let half_jobs = half_jobs(&topologies, &cases, &halves)?;
    let mut outputs = Vec::new();
    wide.evaluate(&wide_jobs, &mut outputs)?;
    half.evaluate(&half_jobs, &mut outputs)?;
    assert_eq!((wide.uploads(), half.uploads()), (500, 500));
    assert_eq!(wide.shards(), half.shards());
    // 100 genomes of each topology, 64 to a shard: two whole shards each.
    let saved: u64 = topologies
        .iter()
        .map(|topology| 2 * 64 * topology.parameter_count() as u64 * 2)
        .sum();
    let (wide_bytes, half_bytes) = (wide.owner().usage().bytes, half.owner().usage().bytes);
    assert_eq!(
        wide_bytes - half_bytes,
        saved,
        "{wide_bytes} vs {half_bytes}"
    );

    let budget = BackendBudget {
        bytes: wide_bytes * 2 / 3,
        organisms: 500,
    };
    let mut tight = MetalBackend::with_budget(64, budget)?;
    tight.evaluate(&wide_jobs, &mut outputs)?;
    let wide_uploads = tight.uploads();
    assert!(wide_uploads < 500, "{wide_uploads} F32 uploads");
    let mut tight = MetalBackend::with_budget(64, budget)?;
    tight.evaluate(&half_jobs, &mut outputs)?;
    assert_eq!(tight.uploads(), 500);
    assert_eq!(bits(&outputs), reference(&half_jobs));
    println!(
        "F32 {wide_bytes} B, F16 {half_bytes} B; under {} B: {wide_uploads} F32 uploads, 500 F16",
        budget.bytes
    );

    // A default shard is `SHARD_BYTES` in either dtype, so it holds twice
    // the binary16 slots: one genome of the preset's topology allocates a
    // whole shard of each.
    let (Some(mut wide), Some(mut half)) = (backend(u64::MAX), backend(u64::MAX)) else {
        return Ok(());
    };
    wide.evaluate(&wide_jobs[2..3], &mut outputs)?;
    half.evaluate(&half_jobs[2..3], &mut outputs)?;
    let parameters = topologies[2].parameter_count() as u64;
    let slots = |bytes: u64| SHARD_BYTES / (bytes * parameters);
    assert_eq!((slots(4), slots(2)), (6_718, 13_437));
    assert_eq!(
        half.owner().usage().bytes as i64 - wide.owner().usage().bytes as i64,
        (slots(2) * parameters * 2) as i64 - (slots(4) * parameters * 4) as i64
    );
    Ok(())
}

#[test]
fn shards_of_any_size_and_slots_past_32_bits_give_the_same_bits() -> Result<(), CoreError> {
    let Some(mut one) = backend(1) else {
        return Ok(());
    };
    let topologies = topologies();
    let cases = cases(&topologies, 120);
    let near = jobs(&topologies, &cases, 0);
    let mut outputs = Vec::new();
    one.evaluate(&near, &mut outputs)?;
    let want = bits(&outputs);
    assert_eq!(
        one.shards(),
        120,
        "one slot to a shard: a kernel per organism"
    );
    for (got, reference) in want.iter().zip(reference(&near)) {
        if got.is_some() {
            assert_eq!(*got, reference);
        }
    }
    for slots in [3, 7, 256] {
        let Some(mut sharded) = backend(slots) else {
            return Ok(());
        };
        sharded.evaluate(&near, &mut outputs)?;
        assert_eq!(bits(&outputs), want, "{slots} slots to a shard");
    }
    for first in [u64::from(u32::MAX) - 7, 1 << 40] {
        let far = jobs(&topologies, &cases, first);
        let Some(mut backend) = backend(256) else {
            return Ok(());
        };
        backend.evaluate(&far, &mut outputs)?;
        assert_eq!(bits(&outputs), want, "slots from {first}");
        assert!(far.iter().all(|job| job.slot.slot() >= first));
    }
    Ok(())
}
