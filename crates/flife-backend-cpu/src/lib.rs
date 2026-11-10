//! FrankenLife's CPU controller backend (plan §10.1, §10.2, §12.5; FL-025):
//! every due organism's own dense controller through FrankenTorch's serial
//! population kernel, `ft_kernel_cpu::population_affine_f32_into`, on the
//! world's own CPU lanes.
//!
//! The kernel computes each output as the bias, then each input's product in
//! ascending index, in F32 with every product and sum rounded and no fused
//! multiply-add: the declared order of
//! [`flife_neural::ReferenceController::evaluate`]. The hidden activation is
//! the reference's own `flife_core::numerics::tanh` of the F32
//! pre-activation, rounded once to F32. So every output is the reference's,
//! bit for bit, and the tests compare bits. The CPU keeps subnormal results,
//! as the reference does, so nothing is deferred for being small.
//!
//! A job is deferred (its output is `None`, and the world evaluates it on the
//! reference) when the reference would refuse it: an input that is not finite
//! in F32, a pre-activation or output that is not finite. It is also deferred
//! when its controller has a shape this backend does not take: not 104 inputs
//! in, 36 outputs out and each layer feeding the next, more than
//! [`MAXIMUM_HIDDEN_LAYERS`] hidden layers, or a parameter count other than
//! the layers'.
//!
//! **Executor** (plan §3.4, §12.5). The kernel runs on the thread that calls
//! it and touches no rayon pool. [`CpuBackend`] splits a batch into at most
//! `lanes` contiguous runs of at least [`MINIMUM_JOBS_PER_LANE`] jobs. The
//! first run is on the calling thread, the rest on scoped threads it joins
//! before returning. So the controllers of a tick use at most `lanes` threads,
//! and the composition root gives it the world's `cpu_workers`.
//!
//! **Residency** (FL-026, plan §9.2). Each catalog member's genomes are
//! packed into that topology's slabs, at their arena slots
//! ([`flife_neural::ArenaSlot`]): fixed-size slots, [`SHARD_SLOTS`] to a
//! shard ([`flife_neural::ShardGeometry`]), each layer's weights transposed
//! to `[inputs][outputs]` (the kernel's `[B, I, O]` with `B = 1`). A genome
//! is packed once, the first time it is due, and kept until a later
//! occupant of its slot (a higher generation) is packed in its place. A
//! batch never drops what it does not contain, so a genome that is due only
//! every K ticks is not packed again each time it is. The slabs follow each
//! topology's peak live count, in whole slots, and a shard's capacity is
//! reserved when it is made, so growing it never moves packed values. A
//! resident slot is used only for the same generation and genome.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::num::NonZeroUsize;

use flife_core::CoreError;
use flife_core::ids::GenomeId;
use flife_core::memory::{bytes_for, capacity_bytes, total};
use flife_core::model::MAXIMUM_HIDDEN_LAYERS;
use flife_core::numerics::tanh;
use flife_neural::{
    ACTION_WIDTH, BackendBudget, ControllerBackend, ControllerJob, DenseTopology, SENSOR_WIDTH,
    ShardGeometry,
};
use ft_kernel_cpu::population_affine_f32_into;

/// The fewest jobs worth handing to another thread, as for the engine's own
/// decision lanes.
pub const MINIMUM_JOBS_PER_LANE: usize = 128;

/// Slots per packed shard, unless [`CpuBackend::with_shard_slots`] says
/// otherwise: 10 MB of the preset's 9,988-parameter topology.
pub const SHARD_SLOTS: u64 = 256;

/// The most F32s one shard holds (1 GiB); a larger topology takes fewer
/// slots to a shard.
pub const MAXIMUM_SHARD_ELEMENTS: u64 = 1 << 28;

// The evaluation below is written out for zero, one or two hidden layers.
const _: () = assert!(MAXIMUM_HIDDEN_LAYERS == 2);

/// One layer of a packed slot: offsets from the slot's first element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layer {
    inputs: usize,
    outputs: usize,
    /// `weights[input][output]`, `inputs · outputs` values.
    weights: usize,
    /// `bias[output]`, `outputs` values.
    biases: usize,
}

impl Layer {
    /// One affine layer of one organism through the kernel.
    fn apply(&self, values: &[f32], source: &[f32], out: &mut [f32]) -> Result<(), CoreError> {
        let weights = values.get(self.weights..self.weights + self.inputs * self.outputs);
        let biases = values.get(self.biases..self.biases + self.outputs);
        let (Some(weights), Some(biases)) = (weights, biases) else {
            return Err(CoreError::StateConflict {
                operation: "cpu controller kernel",
                detail: "a packed layer outside its parameters".to_owned(),
            });
        };
        population_affine_f32_into(source, weights, biases, 1, self.inputs, self.outputs, out)
            .map_err(|error| CoreError::StateConflict {
                operation: "cpu controller kernel",
                detail: error.to_string(),
            })
    }
}

/// The layers of `topology` over `length` parameters, or `None` for a shape
/// this backend does not take.
fn shape(topology: &DenseTopology, length: usize) -> Option<Vec<Layer>> {
    let layers = topology.layers();
    let (last, _) = layers.split_last()?;
    if layers.len() > MAXIMUM_HIDDEN_LAYERS + 1 || last.outputs != ACTION_WIDTH {
        return None;
    }
    let mut fan_in = SENSOR_WIDTH;
    let mut packed = Vec::new();
    let mut offset = 0;
    for layer in layers {
        let weights = layer.inputs.checked_mul(layer.outputs)?;
        let in_bounds = layer.weights.checked_add(weights) == Some(layer.biases)
            && layer.biases.checked_add(layer.outputs)? <= length;
        if layer.inputs != fan_in || layer.inputs == 0 || layer.outputs == 0 || !in_bounds {
            return None;
        }
        packed.push(Layer {
            inputs: layer.inputs,
            outputs: layer.outputs,
            weights: offset,
            biases: offset + weights,
        });
        offset += weights + layer.outputs;
        fan_in = layer.outputs;
    }
    (offset == length).then_some(packed)
}

/// Pack `parameters` (`weight[out][in]` then `bias[out]`, layer by layer)
/// into `out`, in `layers`' order.
fn pack(
    parameters: &[f32],
    topology: &DenseTopology,
    layers: &[Layer],
    out: &mut [f32],
) -> Result<(), CoreError> {
    let outside = || CoreError::LayoutMismatch {
        buffer: "controller parameters",
        detail: format!("a layer outside {} parameters", parameters.len()),
    };
    for (source, layer) in topology.layers().iter().zip(layers) {
        let weights = parameters
            .get(source.weights..source.biases)
            .ok_or_else(outside)?;
        let biases = parameters
            .get(source.biases..source.biases + layer.outputs)
            .ok_or_else(outside)?;
        let packed = out
            .get_mut(layer.weights..layer.biases + layer.outputs)
            .ok_or_else(outside)?;
        let (packed_weights, packed_biases) = packed.split_at_mut(layer.inputs * layer.outputs);
        // Input `i`'s column, `weight[0][i], weight[1][i], …`, becomes the
        // packed row `i`.
        for (input, row) in packed_weights.chunks_exact_mut(layer.outputs).enumerate() {
            for (slot, &value) in row
                .iter_mut()
                .zip(weights.iter().skip(input).step_by(layer.inputs))
            {
                *slot = value;
            }
        }
        packed_biases.copy_from_slice(biases);
    }
    Ok(())
}

/// One organism's 36 raw outputs from its packed slot, or `None` where the
/// reference would refuse it.
fn evaluate(
    layers: &[Layer],
    values: &[f32],
    inputs: &[f64; SENSOR_WIDTH],
    scratch: &mut Scratch,
) -> Result<Option<[f32; ACTION_WIDTH]>, CoreError> {
    let mut rounded = [0.0_f32; SENSOR_WIDTH];
    for (slot, &value) in rounded.iter_mut().zip(inputs) {
        // Rounding each input once to F32 is the declared arithmetic.
        *slot = value as f32;
        if !slot.is_finite() {
            return Ok(None);
        }
    }
    let mut raw = [0.0_f32; ACTION_WIDTH];
    let Some((output, hidden)) = layers.split_last() else {
        return Ok(None);
    };
    let Scratch { first, second } = scratch;
    let activated = match hidden {
        [] => {
            output.apply(values, &rounded, &mut raw)?;
            true
        }
        [one] => {
            let first = &mut first[..one.outputs];
            one.apply(values, &rounded, first)?;
            activate(first) && {
                output.apply(values, first, &mut raw)?;
                true
            }
        }
        [one, two] => {
            let first = &mut first[..one.outputs];
            let second = &mut second[..two.outputs];
            one.apply(values, &rounded, first)?;
            activate(first) && {
                two.apply(values, first, second)?;
                activate(second) && {
                    output.apply(values, second, &mut raw)?;
                    true
                }
            }
        }
        _ => false,
    };
    Ok((activated && raw.iter().all(|value| value.is_finite())).then_some(raw))
}

/// The reference's hidden activation in place: `tanh` of each F32
/// pre-activation in F64, rounded once to F32. False where it refuses one.
fn activate(values: &mut [f32]) -> bool {
    for value in values {
        let Some(activated) = tanh(f64::from(*value)) else {
            return false;
        };
        *value = activated as f32;
    }
    true
}

/// A lane's activations.
#[derive(Debug, Default)]
struct Scratch {
    first: Vec<f32>,
    second: Vec<f32>,
}

impl Scratch {
    /// The bytes its activations hold.
    fn allocated_bytes(&self) -> u64 {
        capacity_bytes(&self.first).saturating_add(capacity_bytes(&self.second))
    }

    /// Room for activations `width` wide.
    fn hold(&mut self, width: usize) -> Result<(), CoreError> {
        for buffer in [&mut self.first, &mut self.second] {
            if buffer.len() < width {
                reserve(buffer, width - buffer.len(), "cpu controller activations")?;
                buffer.resize(width, 0.0);
            }
        }
        Ok(())
    }
}

/// The bytes one shard of `geometry` holds: its values, reserved whole, and
/// a word pair per slot naming its occupant.
const fn shard_bytes(geometry: &ShardGeometry) -> u64 {
    geometry
        .shard_elements()
        .saturating_mul(size_of::<f32>() as u64)
        .saturating_add(
            geometry
                .slots_per_shard()
                .saturating_mul(size_of::<Option<(u32, GenomeId)>>() as u64),
        )
}

/// One shard of a topology's packed slots.
#[derive(Debug, Default)]
struct Shard {
    /// The shard's slots back to back. Its capacity is the whole shard,
    /// reserved when it is made, and it grows a slot at a time within it.
    values: Vec<f32>,
    /// Each slot's packed occupant, its generation and genome, or `None`
    /// before its first.
    occupants: Vec<Option<(u32, GenomeId)>>,
}

/// One catalog member's packed slots, in shards.
#[derive(Debug)]
struct Packing {
    layers: Vec<Layer>,
    widest: usize,
    geometry: ShardGeometry,
    shards: HashMap<u64, Shard>,
}

impl Packing {
    /// The packed values of `job`'s slot, if they are its generation's and
    /// genome's.
    fn resident(&self, job: &ControllerJob<'_>) -> Option<&[f32]> {
        let at = self.geometry.locate(job.slot.slot());
        let shard = self.shards.get(&at.shard)?;
        let local = usize::try_from(at.slot_in_shard).ok()?;
        let occupant = shard.occupants.get(local).copied().flatten()?;
        if occupant != (job.slot.generation(), job.genome) {
            return None;
        }
        let offset = usize::try_from(at.offset).ok()?;
        let length = usize::try_from(at.length).ok()?;
        shard.values.get(offset..offset.checked_add(length)?)
    }

    /// Pack `job`'s parameters into its slot, in place of any earlier
    /// occupant's, unless they are resident, or their shard is not made yet
    /// and would need more than `room` bytes (FL-043). Whether it packed
    /// them, and the bytes of any shard it made.
    fn place(&mut self, job: &ControllerJob<'_>, room: u64) -> Result<(bool, u64), CoreError> {
        if self.resident(job).is_some() {
            return Ok((false, 0));
        }
        let too_large = |quantity: &'static str| CoreError::Overflow { quantity };
        let at = self.geometry.locate(job.slot.slot());
        let local = usize::try_from(at.slot_in_shard).map_err(|_| too_large("arena slot"))?;
        let offset = usize::try_from(at.offset).map_err(|_| too_large("arena offset"))?;
        let length = usize::try_from(at.length).map_err(|_| too_large("arena slot length"))?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| too_large("arena offset"))?;
        let shard_elements = usize::try_from(self.geometry.shard_elements())
            .map_err(|_| too_large("arena shard elements"))?;
        let slots = usize::try_from(self.geometry.slots_per_shard())
            .map_err(|_| too_large("arena shard slots"))?;
        let mut made = 0;
        let shard = match self.shards.entry(at.shard) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let bytes = shard_bytes(&self.geometry);
                if bytes > room {
                    return Ok((false, 0));
                }
                let mut shard = Shard::default();
                shard
                    .values
                    .try_reserve_exact(shard_elements)
                    .map_err(|_| CoreError::AllocationFailed {
                        buffer: "cpu controller shard",
                        bytes: self.geometry.shard_elements().saturating_mul(4),
                    })?;
                shard.occupants.try_reserve_exact(slots).map_err(|_| {
                    CoreError::AllocationFailed {
                        buffer: "cpu controller slots",
                        bytes: bytes_for::<Option<(u32, GenomeId)>>(slots),
                    }
                })?;
                made = bytes;
                entry.insert(shard)
            }
        };
        if shard.values.len() < end {
            shard.values.resize(end, 0.0);
        }
        if shard.occupants.len() <= local {
            let more = local + 1 - shard.occupants.len();
            reserve(&mut shard.occupants, more, "cpu controller slots")?;
            shard.occupants.resize(local + 1, None);
        }
        // The old occupant is gone before anything of the new one is
        // written, so a failure leaves the slot empty, never mixed.
        shard.occupants[local] = None;
        pack(
            job.controller.parameters(),
            job.controller.topology(),
            &self.layers,
            &mut shard.values[offset..end],
        )?;
        shard.occupants[local] = Some((job.slot.generation(), job.genome));
        Ok((true, made))
    }
}

/// One catalog member, as this backend first saw it.
#[derive(Debug)]
struct Bucket {
    topology: DenseTopology,
    /// `None` for a shape this backend does not take: its jobs are
    /// deferred.
    packing: Option<Packing>,
}

impl Bucket {
    /// The packing that serves `job`, if its topology is this bucket's and
    /// its shape one this backend takes.
    fn serving(&self, job: &ControllerJob<'_>) -> Option<&Packing> {
        let packing = self.packing.as_ref()?;
        let length = job.controller.parameters().len() as u64;
        (*job.controller.topology() == self.topology && length == packing.geometry.parameters())
            .then_some(packing)
    }
}

/// The CPU controller backend: FrankenTorch's serial population kernel on at
/// most `lanes` threads.
#[derive(Debug)]
pub struct CpuBackend {
    lanes: NonZeroUsize,
    shard_slots: u64,
    budget: BackendBudget,
    /// The bytes its shards hold.
    held: u64,
    buckets: Vec<Option<Bucket>>,
    scratch: Vec<Scratch>,
    /// The threads the last batch ran on.
    used: usize,
    packs: u64,
}

impl CpuBackend {
    /// A backend evaluating on at most `lanes` threads, the calling one
    /// included. Give it the world's `cpu_workers`.
    #[must_use]
    pub const fn new(lanes: NonZeroUsize) -> Self {
        Self::with_shard_slots(lanes, SHARD_SLOTS)
    }

    /// As [`CpuBackend::new`], with at most `shard_slots` slots to a shard
    /// (at least one).
    #[must_use]
    pub const fn with_shard_slots(lanes: NonZeroUsize, shard_slots: u64) -> Self {
        Self::with_budget(lanes, shard_slots, BackendBudget::UNBOUNDED)
    }

    /// As [`CpuBackend::with_shard_slots`], holding at most `budget.bytes`
    /// (FL-043, plan §19.4): its shards and its lanes' activations. A shard
    /// that would leave too little for the lanes' activations at the widest
    /// topology served is not made, and its jobs are deferred to the
    /// reference, which gives the same bits.
    #[must_use]
    pub const fn with_budget(lanes: NonZeroUsize, shard_slots: u64, budget: BackendBudget) -> Self {
        Self {
            lanes,
            shard_slots: if shard_slots == 0 { 1 } else { shard_slots },
            budget,
            held: 0,
            buckets: Vec::new(),
            scratch: Vec::new(),
            used: 0,
            packs: 0,
        }
    }

    /// The most threads one batch uses.
    #[must_use]
    pub const fn lanes(&self) -> NonZeroUsize {
        self.lanes
    }

    /// The threads the last batch ran on, the calling one included: none
    /// for an empty batch, otherwise at most [`lanes`](Self::lanes) and at
    /// most one per [`MINIMUM_JOBS_PER_LANE`] jobs.
    #[must_use]
    pub const fn threads_used(&self) -> usize {
        self.used
    }

    /// Slots holding a packed genome. A dead genome's slot stays packed
    /// until a newborn takes it, so this follows each topology's peak live
    /// count, not the batch.
    #[must_use]
    pub fn resident(&self) -> usize {
        self.buckets
            .iter()
            .flatten()
            .filter_map(|bucket| bucket.packing.as_ref())
            .flat_map(|packing| packing.shards.values())
            .map(|shard| shard.occupants.iter().flatten().count())
            .sum()
    }

    /// Shards across every topology.
    #[must_use]
    pub fn shards(&self) -> usize {
        self.buckets
            .iter()
            .flatten()
            .filter_map(|bucket| bucket.packing.as_ref())
            .map(|packing| packing.shards.len())
            .sum()
    }

    /// Genomes packed since the backend was made or last forgot: once per
    /// genome while it lives, however often it is due.
    #[must_use]
    pub const fn packs(&self) -> u64 {
        self.packs
    }

    /// Pack every job's parameters that are not resident and whose shard
    /// fits the budget, and give the widest hidden layer among them.
    fn place(&mut self, jobs: &[ControllerJob<'_>]) -> Result<usize, CoreError> {
        let mut widest = 0;
        let lanes = self.lanes.get() as u64;
        for job in jobs {
            let Ok(index) = usize::try_from(job.slot.topology()) else {
                continue;
            };
            if self.buckets.len() <= index {
                let more = index + 1 - self.buckets.len();
                reserve(&mut self.buckets, more, "cpu controller buckets")?;
                self.buckets.resize_with(index + 1, || None);
            }
            let topology = job.controller.topology();
            let length = job.controller.parameters().len();
            let bucket = self.buckets[index].get_or_insert_with(|| Bucket {
                topology: topology.clone(),
                packing: shape(topology, length).and_then(|layers| {
                    let geometry = ShardGeometry::fitting(
                        length as u64,
                        MAXIMUM_SHARD_ELEMENTS,
                        self.shard_slots,
                    )
                    .ok()?;
                    let widest = layers.split_last().map_or(0, |(_, hidden)| {
                        hidden.iter().map(|layer| layer.outputs).max().unwrap_or(0)
                    });
                    Some(Packing {
                        layers,
                        widest,
                        geometry,
                        shards: HashMap::new(),
                    })
                }),
            });
            if bucket.serving(job).is_none() {
                continue;
            }
            let Some(packing) = bucket.packing.as_mut() else {
                continue;
            };
            // The lanes' activations: two buffers of the widest layer served,
            // or what they already hold.
            let activations = lanes
                .saturating_mul(2)
                .saturating_mul(bytes_for::<f32>(widest.max(packing.widest)))
                .max(total(self.scratch.iter().map(Scratch::allocated_bytes)));
            let room = self
                .budget
                .bytes
                .saturating_sub(self.held.saturating_add(activations));
            let (packed, made) = packing.place(job, room)?;
            self.held = self.held.saturating_add(made);
            if packed {
                self.packs += 1;
            }
            if packing.resident(job).is_some() {
                widest = widest.max(packing.widest);
            }
        }
        Ok(widest)
    }
}

/// Evaluate a run of jobs into its outputs.
fn run(
    buckets: &[Option<Bucket>],
    jobs: &[ControllerJob<'_>],
    outputs: &mut [Option<[f32; ACTION_WIDTH]>],
    scratch: &mut Scratch,
) -> Result<(), CoreError> {
    for (job, output) in jobs.iter().zip(outputs) {
        let packing = usize::try_from(job.slot.topology())
            .ok()
            .and_then(|index| buckets.get(index))
            .and_then(Option::as_ref)
            .and_then(|bucket| bucket.serving(job));
        *output = match packing.and_then(|packing| Some((packing, packing.resident(job)?))) {
            Some((packing, values)) => evaluate(&packing.layers, values, job.inputs, scratch)?,
            None => None,
        };
    }
    Ok(())
}

impl ControllerBackend for CpuBackend {
    fn describe(&self) -> String {
        format!(
            "cpu (frankentorch population_affine_f32_into, {} lanes)",
            self.lanes
        )
    }

    fn resident_bytes(&self) -> u64 {
        let shards = self
            .buckets
            .iter()
            .flatten()
            .filter_map(|bucket| bucket.packing.as_ref())
            .flat_map(|packing| packing.shards.values())
            .map(|shard| {
                capacity_bytes(&shard.values).saturating_add(capacity_bytes(&shard.occupants))
            });
        total(shards.chain(self.scratch.iter().map(Scratch::allocated_bytes)))
    }

    fn forget(&mut self) {
        self.buckets.clear();
        self.held = 0;
        self.packs = 0;
    }

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
    ) -> Result<(), CoreError> {
        self.used = 0;
        outputs.clear();
        reserve(outputs, jobs.len(), "cpu controller outputs")?;
        outputs.resize(jobs.len(), None);
        let widest = self.place(jobs)?;
        if jobs.is_empty() {
            return Ok(());
        }
        let lanes = self
            .lanes
            .get()
            .min(jobs.len().div_ceil(MINIMUM_JOBS_PER_LANE))
            .max(1);
        if let Some(missing) = lanes.checked_sub(self.scratch.len()).filter(|&n| n > 0) {
            reserve(&mut self.scratch, missing, "cpu controller lanes")?;
            self.scratch.resize_with(lanes, Scratch::default);
        }
        for scratch in &mut self.scratch[..lanes] {
            scratch.hold(widest)?;
        }
        let buckets = &self.buckets;
        let length = jobs.len().div_ceil(lanes);
        let mut runs = jobs
            .chunks(length)
            .zip(outputs.chunks_mut(length))
            .zip(self.scratch.iter_mut());
        let Some(((head_jobs, head_outputs), head_scratch)) = runs.next() else {
            return Ok(());
        };
        if lanes == 1 {
            self.used = 1;
            return run(buckets, head_jobs, head_outputs, head_scratch);
        }
        let (result, used) = std::thread::scope(|scope| {
            let workers: Vec<_> = runs
                .map(|((jobs, outputs), scratch)| {
                    scope.spawn(move || run(buckets, jobs, outputs, scratch))
                })
                .collect();
            let used = 1 + workers.len();
            let mut result = run(buckets, head_jobs, head_outputs, head_scratch);
            for worker in workers {
                let joined = worker.join().unwrap_or_else(|_| {
                    Err(CoreError::StateConflict {
                        operation: "cpu controller lanes",
                        detail: "a controller lane panicked".to_owned(),
                    })
                });
                if result.is_ok() {
                    result = joined;
                }
            }
            (result, used)
        });
        self.used = used;
        result
    }
}

fn reserve<T>(
    vector: &mut Vec<T>,
    additional: usize,
    buffer: &'static str,
) -> Result<(), CoreError> {
    vector
        .try_reserve(additional)
        .map_err(|_| CoreError::AllocationFailed {
            buffer,
            bytes: u64::try_from(additional.saturating_mul(size_of::<T>())).unwrap_or(u64::MAX),
        })
}
