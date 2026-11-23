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
    ArenaSlot, ControllerBackend, ControllerJob, ControllerScratch, DenseTopology, RawOutputs,
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

type Bits = Vec<Option<Vec<u32>>>;

fn bits(outputs: &[Option<RawOutputs>]) -> Bits {
    outputs
        .iter()
        .map(|output| output.map(|raw| raw.iter().map(|v| v.to_bits()).collect()))
        .collect()
}

/// The reference's outputs for each job.
#[expect(clippy::expect_used, reason = "test helper")]
fn reference(jobs: &[ControllerJob<'_>]) -> Bits {
    jobs.iter()
        .map(|job| {
            let mut scratch = ControllerScratch::new(job.controller.topology()).expect("scratch");
            job.controller
                .evaluate_raw(job.inputs, &mut scratch)
                .ok()
                .map(|raw| raw.iter().map(|v| v.to_bits()).collect())
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
    assert_eq!(bits(&outputs), [None, first[0].clone()]);

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
    // inputs, outputs, fault flags and dims buffer, and nothing else.
    let usage = backend.owner().usage();
    assert_eq!(usage.buffers, 2 * backend.shards() + 4, "{usage:?}");

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

/// FL-043: retiring every live occupant of a Metal device shard drops the
/// shard's parameter and job buffers immediately, reducing `shards()`,
/// `resident_genomes()`, `resident_bytes()`, and `owner().usage()`, and
/// freeing budget headroom for later shards.
#[test]
fn empty_shards_are_reclaimed_on_device_when_slots_retire() -> Result<(), CoreError> {
    let Some(mut backend) = backend(4) else {
        return Ok(());
    };
    let topologies = topologies();
    let cases = cases(&topologies, 40);
    let jobs = jobs(&topologies, &cases, 0);
    let mut outputs = Vec::new();
    backend.evaluate(&jobs, &mut outputs)?;
    let first = bits(&outputs);
    assert_eq!(backend.shards(), 10);
    assert_eq!(backend.resident_genomes(), 40);
    let peak_resident = backend.resident_bytes();
    let peak_usage = backend.owner().usage();
    assert_eq!(peak_usage.buffers, 2 * 10 + 4);
    let initial_uploads = backend.uploads();

    // Wrong generation must leave every live slot and shard intact.
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
    assert_eq!(backend.shards(), 10);
    assert_eq!(backend.resident_genomes(), 40);

    // Retire shard 1 (slot >= 4) of all 5 topologies.
    let shard1_slots: Vec<ArenaSlot> = jobs
        .iter()
        .filter(|job| job.slot.slot() >= 4)
        .map(|job| job.slot)
        .collect();
    backend.release_slots(&shard1_slots);
    assert_eq!(backend.shards(), 5);
    assert_eq!(backend.resident_genomes(), 20);
    let after_usage = backend.owner().usage();
    assert_eq!(
        after_usage.buffers,
        2 * 5 + 4,
        "dropped shards must release both parameter and job device buffers"
    );
    assert!(
        after_usage.bytes < peak_usage.bytes && backend.resident_bytes() < peak_resident,
        "device bytes ({} vs {}) and resident bytes ({} vs {}) must fall",
        after_usage.bytes,
        peak_usage.bytes,
        backend.resident_bytes(),
        peak_resident
    );

    // Surviving shard-0 jobs stay resident (zero re-uploads) and bit-identical.
    let shard0_jobs: Vec<ControllerJob<'_>> = jobs
        .iter()
        .copied()
        .filter(|job| job.slot.slot() < 4)
        .collect();
    backend.evaluate(&shard0_jobs, &mut outputs)?;
    let expected0: Bits = jobs
        .iter()
        .zip(&first)
        .filter(|(job, _)| job.slot.slot() < 4)
        .map(|(_, out)| out.clone())
        .collect();
    assert_eq!(bits(&outputs), expected0);
    assert_eq!(backend.uploads(), initial_uploads);

    // Retiring shard 0 reclaims the remaining 5 shards too.
    let shard0_slots: Vec<ArenaSlot> = shard0_jobs.iter().map(|job| job.slot).collect();
    backend.release_slots(&shard0_slots);
    assert_eq!(backend.shards(), 0);
    assert_eq!(backend.resident_genomes(), 0);
    assert_eq!(backend.owner().usage().buffers, 4);
    Ok(())
}

/// The specialized binary16 forward pass checks layer inputs once against
/// `2^-78` (`HALF_TINY_INPUT_BITS`) instead of per product: a nonzero input
/// below `2^-78` can make a binary16 subnormal weight (`2^-24`) produce a
/// subnormal product (`< 2^-102`) and must defer, while `2^-77` keeps every
/// nonzero product at or above `2^-101` and must execute bit-identically on
/// the device.
#[test]
fn half_kernel_defers_tiny_inputs_below_two_to_the_minus_78() -> Result<(), CoreError> {
    let Some(mut backend) = backend(16) else {
        return Ok(());
    };
    let topology = DenseTopology::new(&[8])?;
    // Smallest positive binary16 subnormal is 2^-24 (raw bits 0x0001).
    let tiny_weight = F16::from_bits(0x0001);
    let mut parameters = vec![F16::ZERO; topology.parameter_count()];
    for layer in topology.layers() {
        for value in &mut parameters[layer.weights..layer.biases] {
            *value = tiny_weight;
        }
        // Keep layer 0's post-bias activation O(1) so layer 1's inputs stay
        // far above 2^-78, isolating the 2^-78 threshold check to layer 0.
        for bias in &mut parameters[layer.biases..layer.biases + layer.outputs] {
            *bias = F16::from_f64(1.0);
        }
    }
    let mut below = [0.0_f64; SENSOR_WIDTH];
    below[0] = 2.0_f64.powi(-80);
    let mut above = [0.0_f64; SENSOR_WIDTH];
    above[0] = 2.0_f64.powi(-77);

    let jobs = vec![
        ControllerJob {
            genome: GenomeId::new(1),
            slot: ArenaSlot::new(0, 0, 1),
            controller: ReferenceController::new(&topology, &parameters, BOUND)?,
            inputs: &below,
        },
        ControllerJob {
            genome: GenomeId::new(2),
            slot: ArenaSlot::new(0, 1, 1),
            controller: ReferenceController::new(&topology, &parameters, BOUND)?,
            inputs: &above,
        },
    ];
    let want = reference(&jobs);
    let mut outputs = Vec::new();
    backend.evaluate(&jobs, &mut outputs)?;
    assert_eq!(
        outputs[0], None,
        "nonzero input 2^-80 < 2^-78 must defer in the binary16 kernel"
    );
    assert!(
        outputs[1].is_some(),
        "nonzero input 2^-77 >= 2^-78 must execute on the device"
    );
    assert_eq!(bits(&outputs)[1], want[1]);
    Ok(())
}

/// Hidden widths that are not multiples of 4 exercise the scalar fallback
/// loop in both `forward_pass_f32` and `forward_pass_half`, and Trap A's
/// 4-element F16 shard stride guarantees even odd parameter counts (`[5]` =
/// 741 params -> 744-element F16 stride) execute on GPU with 0 deferrals.
#[test]
fn non_multiple_of_four_hidden_widths_match_reference_in_f32_and_f16() -> Result<(), CoreError> {
    let (Some(mut wide), Some(mut half)) = (backend(16), backend(16)) else {
        return Ok(());
    };
    let custom = [
        DenseTopology::new(&[6])?,
        DenseTopology::new(&[10, 6])?,
        DenseTopology::new(&[5])?,
    ];
    let mut draws = Draws(4242);
    let mut f32_params = Vec::new();
    let mut f16_params = Vec::new();
    let mut inputs_list = Vec::new();
    for topology in &custom {
        let mut params = vec![0.0_f32; topology.parameter_count()];
        for layer in topology.layers() {
            let scale = 1.0 / (layer.inputs as f64).sqrt();
            for value in &mut params[layer.weights..layer.biases + layer.outputs] {
                *value = (draws.symmetric() * scale) as f32;
            }
        }
        let h: Vec<F16> = params
            .iter()
            .map(|&v| F16::from_f64(f64::from(v)))
            .collect();
        let mut inp = [0.0; SENSOR_WIDTH];
        for v in &mut inp {
            *v = draws.symmetric() * 2.0;
        }
        f32_params.push(params);
        f16_params.push(h);
        inputs_list.push(inp);
    }

    let wide_jobs: Vec<ControllerJob<'_>> = (0..custom.len())
        .map(|i| {
            Ok(ControllerJob {
                genome: GenomeId::new(i as u64 + 1),
                slot: ArenaSlot::new(i as u32, 0, 1),
                controller: ReferenceController::new(&custom[i], &f32_params[i], BOUND)?,
                inputs: &inputs_list[i],
            })
        })
        .collect::<Result<_, CoreError>>()?;
    let half_jobs: Vec<ControllerJob<'_>> = (0..custom.len())
        .map(|i| {
            Ok(ControllerJob {
                genome: GenomeId::new(i as u64 + 100),
                slot: ArenaSlot::new(i as u32, 0, 1),
                controller: ReferenceController::new(&custom[i], &f16_params[i], BOUND)?,
                inputs: &inputs_list[i],
            })
        })
        .collect::<Result<_, CoreError>>()?;

    let mut wide_out = Vec::new();
    wide.evaluate(&wide_jobs, &mut wide_out)?;
    assert_eq!(bits(&wide_out), reference(&wide_jobs));

    let mut half_out = Vec::new();
    half.evaluate(&half_jobs, &mut half_out)?;
    let half_want = reference(&half_jobs);
    // [6] (882 params), [10, 6] (1368 params), and odd [5] (741 params,
    // padded to 744-element F16 stride) all execute on GPU with 0 deferrals.
    assert!(
        half_out.iter().all(Option::is_some),
        "expected 0 F16 alignment deferrals across [6], [10, 6], [5]"
    );
    assert_eq!(bits(&half_out), half_want);
    Ok(())
}

/// V2-01 cross-backend bit-exact parity across Reference, FrankenTorch CPU
/// (`CpuBackend`), and Metal (`MetalControllers` + `MetalBackend` F32 & F16):
/// covers `(104, 36)`, `(104, 40)`, `(108, 44)`, `(112, 48)`, and odd
/// `(105, 37)` across 1-hidden, 2-hidden, and odd-width topologies with
/// **0 F16 alignment deferrals** (Trap A 4-element F16 stride + Trap B
/// `sensed4[WIDEST / 4]`), plus recurrent `update_recurrent_state` parity.
#[test]
fn variable_width_reference_torch_and_metal_bit_exact() -> Result<(), CoreError> {
    use std::num::NonZeroUsize;

    use flife_backend_cpu::CpuBackend;
    use flife_backend_metal::{Batch, MetalControllers};

    let cases_dims: [(u32, u32, &[&[u32]]); 5] = [
        (104, 36, &[&[8], &[64, 32]]),
        (104, 40, &[&[8], &[64, 32]]),
        (108, 44, &[&[16], &[64, 32]]),
        (112, 48, &[&[32], &[64, 32]]),
        (105, 37, &[&[5], &[8], &[32, 16]]),
    ];

    let mut draws = Draws(0x2026_0201);
    let metal_direct = match MetalControllers::open() {
        Ok(controllers) => Some(controllers),
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => None,
        Err(error) => return Err(error),
    };
    let mut metal_f32 = backend(8);
    let mut metal_f16 = backend(8);

    for (dim_idx, &(in_w, out_w, hidden_list)) in cases_dims.iter().enumerate() {
        let topos: Vec<DenseTopology> = hidden_list
            .iter()
            .map(|h| DenseTopology::with_io(in_w, h, out_w))
            .collect::<Result<_, _>>()?;

        let per_topo = 10_usize;
        let total = topos.len() * per_topo;
        let mut f32_params = Vec::with_capacity(total);
        let mut f16_params = Vec::with_capacity(total);
        let mut inputs_list = Vec::with_capacity(total);
        let mut topo_indices = Vec::with_capacity(total);
        for (t_idx, topo) in topos.iter().enumerate() {
            for _ in 0..per_topo {
                let mut p = vec![0.0_f32; topo.parameter_count()];
                for layer in topo.layers() {
                    let scale = 1.0 / (layer.inputs as f64).sqrt();
                    for v in &mut p[layer.weights..layer.biases + layer.outputs] {
                        // Avoid tiny weights near 2^-78 so normal Metal jobs never
                        // trigger the subnormal deferral path and we verify 0 deferrals.
                        let raw = draws.symmetric();
                        let nudged = if raw.abs() < 1.0e-3 { 0.25 } else { raw };
                        *v = (nudged * scale) as f32;
                    }
                }
                let h: Vec<F16> = p.iter().map(|&v| F16::from_f64(f64::from(v))).collect();
                let inp: Vec<f64> = (0..in_w as usize)
                    .map(|_| {
                        let raw = draws.symmetric();
                        let nudged = if raw.abs() < 1.0e-3 { 0.5 } else { raw };
                        nudged * 1.5
                    })
                    .collect();
                f32_params.push(p);
                f16_params.push(h);
                inputs_list.push(inp);
                topo_indices.push(t_idx);
            }
        }

        // Build jobs for F32 and F16.
        let mut next_slot = vec![0_u64; topos.len()];
        let mut jobs_f32 = Vec::with_capacity(total);
        let mut jobs_f16 = Vec::with_capacity(total);
        let mut direct_batch = Batch::new();
        for i in 0..total {
            let t = topo_indices[i];
            let slot = next_slot[t];
            next_slot[t] += 1;
            let bucket_id = (dim_idx * 8 + t) as u32;
            jobs_f32.push(ControllerJob {
                genome: GenomeId::new((dim_idx as u64 + 1) * 10_000 + i as u64),
                slot: ArenaSlot::new(bucket_id, slot, 1),
                controller: ReferenceController::new(&topos[t], &f32_params[i], BOUND)?,
                inputs: &inputs_list[i],
            });
            jobs_f16.push(ControllerJob {
                genome: GenomeId::new((dim_idx as u64 + 1) * 20_000 + i as u64),
                slot: ArenaSlot::new(bucket_id, slot, 1),
                controller: ReferenceController::new(&topos[t], &f16_params[i], BOUND)?,
                inputs: &inputs_list[i],
            });
            direct_batch.push(&topos[t], &f32_params[i], &inputs_list[i])?;
        }

        // 1. Reference oracle.
        let want_f32 = reference(&jobs_f32);
        let want_f16 = reference(&jobs_f16);
        assert!(want_f32.iter().all(Option::is_some));
        assert!(want_f16.iter().all(Option::is_some));

        // 2. FrankenTorch CPU backend (F32 & F16).
        let lanes = NonZeroUsize::new(2).unwrap_or(NonZeroUsize::MIN);
        let mut cpu = CpuBackend::new(lanes);
        let mut out_cpu_f32 = Vec::new();
        cpu.evaluate(&jobs_f32, &mut out_cpu_f32)?;
        assert_eq!(
            bits(&out_cpu_f32),
            want_f32,
            "CpuBackend F32 mismatch for ({in_w}, {out_w})"
        );
        let mut cpu_f16 = CpuBackend::new(lanes);
        let mut out_cpu_f16 = Vec::new();
        cpu_f16.evaluate(&jobs_f16, &mut out_cpu_f16)?;
        assert_eq!(
            bits(&out_cpu_f16),
            want_f16,
            "CpuBackend F16 mismatch for ({in_w}, {out_w})"
        );

        // 3. MetalControllers direct Batch (F32) + MetalBackend resident F32 & F16.
        if let Some(controllers) = metal_direct.as_ref() {
            let mut out_direct = Vec::new();
            controllers.evaluate(&direct_batch, &mut out_direct)?;
            assert!(
                out_direct.iter().all(Option::is_some),
                "MetalControllers deferred a job for ({in_w}, {out_w})"
            );
            assert_eq!(
                bits(&out_direct),
                want_f32,
                "MetalControllers F32 mismatch for ({in_w}, {out_w})"
            );
        }
        if let Some(backend_f32) = metal_f32.as_mut() {
            let mut out_m32 = Vec::new();
            backend_f32.evaluate(&jobs_f32, &mut out_m32)?;
            assert!(
                out_m32.iter().all(Option::is_some),
                "MetalBackend F32 deferred a job for ({in_w}, {out_w})"
            );
            assert_eq!(
                bits(&out_m32),
                want_f32,
                "MetalBackend F32 mismatch for ({in_w}, {out_w})"
            );
        }
        if let Some(backend_f16) = metal_f16.as_mut() {
            let uploads_before = backend_f16.uploads();
            let mut out_m16 = Vec::new();
            backend_f16.evaluate(&jobs_f16, &mut out_m16)?;
            let deferred_f16 = out_m16.iter().filter(|o| o.is_none()).count();
            assert_eq!(
                deferred_f16, 0,
                "MetalBackend F16 had {deferred_f16} alignment deferrals for ({in_w}, {out_w})"
            );
            assert_eq!(
                backend_f16.uploads() - uploads_before,
                total as u64,
                "every F16 genome must upload for ({in_w}, {out_w})"
            );
            assert_eq!(
                bits(&out_m16),
                want_f16,
                "MetalBackend F16 mismatch for ({in_w}, {out_w})"
            );
        }
    }

    // Verify Bundle A recurrent (106 sensors + S=2 -> [64, 32] -> 40 actions + 2*2 tail = 108/44)
    // matches ReferenceController::evaluate_recurrent_slice bit-for-bit across
    // CpuBackend and MetalBackend F16/F32 using shared `update_recurrent_state`.
    let rec_topo = DenseTopology::with_io_recurrent(106, &[64, 32], 40, 2)?;
    let mut rec_f32 = vec![0.0_f32; rec_topo.parameter_count()];
    for layer in rec_topo.layers() {
        let scale = 1.0 / (layer.inputs as f64).sqrt();
        for v in &mut rec_f32[layer.weights..layer.biases + layer.outputs] {
            let raw = draws.symmetric();
            *v = (if raw.abs() < 1.0e-3 { 0.25 } else { raw } * scale) as f32;
        }
    }
    let rec_f16: Vec<F16> = rec_f32
        .iter()
        .map(|&v| F16::from_f64(f64::from(v)))
        .collect();
    let sensor_in: Vec<f64> = (0..106)
        .map(|_| {
            let raw = draws.symmetric();
            if raw.abs() < 1.0e-3 { 0.5 } else { raw }
        })
        .collect();
    let state = [0.45_f32, -0.35];
    let mut combined_in = sensor_in.clone();
    combined_in.extend(state.iter().map(|&h| f64::from(h)));

    let ctrl_f16 = ReferenceController::new(&rec_topo, &rec_f16, BOUND)?;
    let mut scratch = ControllerScratch::new(&rec_topo)?;
    let mut ref_next = [0.0_f32; 2];
    let ref_actions =
        ctrl_f16.evaluate_recurrent_slice(&sensor_in, &state, 2.5, &mut scratch, &mut ref_next)?;

    let rec_job = ControllerJob {
        genome: GenomeId::new(999_001),
        slot: ArenaSlot::new(63, 0, 1),
        controller: ctrl_f16,
        inputs: &combined_in,
    };
    let mut cpu = CpuBackend::new(NonZeroUsize::MIN);
    let mut cpu_out = Vec::new();
    cpu.evaluate(&[rec_job], &mut cpu_out)?;
    let cpu_raw = cpu_out[0].expect("recurrent F16 on CpuBackend");
    let mut cpu_next = [0.0_f32; 2];
    flife_neural::update_recurrent_state(&state, &cpu_raw[40..44], 2.5, &mut cpu_next)?;
    assert_eq!(
        cpu_raw[..40]
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        ref_actions.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    assert_eq!(cpu_next.map(f32::to_bits), ref_next.map(f32::to_bits));

    if let Some(backend_f16) = metal_f16.as_mut() {
        let mut metal_out = Vec::new();
        backend_f16.evaluate(&[rec_job], &mut metal_out)?;
        let metal_raw = metal_out[0].expect("recurrent F16 on MetalBackend");
        let mut metal_next = [0.0_f32; 2];
        flife_neural::update_recurrent_state(&state, &metal_raw[40..44], 2.5, &mut metal_next)?;
        assert_eq!(
            metal_raw[..40]
                .iter()
                .map(|v| v.to_bits())
                .collect::<Vec<_>>(),
            ref_actions.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
        assert_eq!(metal_next.map(f32::to_bits), ref_next.map(f32::to_bits));
    }
    Ok(())
}
