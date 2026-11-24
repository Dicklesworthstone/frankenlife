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
//! pre-activation, rounded once to F32, computed as the reference computes
//! it (`flife_core::numerics::tanh_f32`). So every output is the reference's,
//! bit for bit, and the tests compare bits. The CPU keeps subnormal results,
//! as the reference does, so nothing is deferred for being small.
//!
//! A job is deferred (its output is `None`, and the world evaluates it on the
//! reference) when the reference would refuse it: an input that is not finite
//! in F32, a pre-activation or output that is not finite. It is also deferred
//! when its controller has a shape this backend does not take: input width
//! outside `1..=MAX_SENSOR_WIDTH`, raw output width outside
//! `1..=MAX_CONTROLLER_OUTPUT_WIDTH`, mismatched input slice length, layers
//! that do not feed the next, more than [`MAXIMUM_HIDDEN_LAYERS`] hidden
//! layers, or a parameter count other than the layers'.
//!
//! **Executor** (plan §3.4, §12.5). The kernel runs on the thread that calls
//! it and touches no global rayon pool. [`CpuBackend`] splits a batch into at
//! most `lanes` contiguous runs of at least [`MINIMUM_JOBS_PER_LANE`] jobs. The
//! first run is on the calling thread, the rest on the cached private
//! [`flife_core::pool`] sized to `lanes` total threads (joined before
//! returning). So the controllers of a tick use at most `lanes` threads with
//! zero per-tick OS thread spawns, and the composition root gives it the
//! world's `cpu_workers`.
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
use std::num::NonZeroUsize;

use flife_core::ids::GenomeId;
use flife_core::memory::{bytes_for, capacity_bytes, total};
use flife_core::model::MAXIMUM_HIDDEN_LAYERS;
use flife_core::numerics::tanh_f32;
use flife_core::{CoreError, pool};
use flife_neural::{
    ArenaSlot, BackendBudget, ControllerBackend, ControllerJob, DenseTopology, FlatControllerJob,
    FlatOutputs, MAX_CONTROLLER_OUTPUT_WIDTH, MAX_SENSOR_WIDTH, RawOutputs, ReferenceController,
    ShardGeometry, Weight, Weights,
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

/// Direct-indexed shard slots below this threshold (`0..2048` covers up to
/// 524,288 slots per topology at default `SHARD_SLOTS = 256` with zero
/// hashing); higher sparse shard IDs fall back to `HashMap`.
const DENSE_SHARDS: usize = 2_048;

/// Fixed per-topology intra-batch pointer cache slots (`0..8` covers the entire
/// 5-topology catalog with zero heap allocation).
const TOPO_CACHE_SLOTS: usize = 8;

/// Uniform metadata view over [`ControllerJob`] and [`FlatControllerJob`] so
/// resident placement and evaluation share one zero-duplication core.
trait JobView {
    fn genome(&self) -> GenomeId;
    fn slot(&self) -> ArenaSlot;
    fn controller(&self) -> ReferenceController<'_>;
}

impl JobView for ControllerJob<'_> {
    #[inline]
    fn genome(&self) -> GenomeId {
        self.genome
    }

    #[inline]
    fn slot(&self) -> ArenaSlot {
        self.slot
    }

    #[inline]
    fn controller(&self) -> ReferenceController<'_> {
        self.controller
    }
}

impl JobView for FlatControllerJob<'_> {
    #[inline]
    fn genome(&self) -> GenomeId {
        self.genome
    }

    #[inline]
    fn slot(&self) -> ArenaSlot {
        self.slot
    }

    #[inline]
    fn controller(&self) -> ReferenceController<'_> {
        self.controller
    }
}

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
    let input_width = topology.input_width();
    let output_width = topology.raw_output_width();
    if layers.len() > MAXIMUM_HIDDEN_LAYERS + 1
        || input_width == 0
        || input_width > MAX_SENSOR_WIDTH
        || output_width == 0
        || output_width > MAX_CONTROLLER_OUTPUT_WIDTH
        || last.outputs != output_width
    {
        return None;
    }
    let mut fan_in = input_width;
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
/// into `out`, in `layers`' order, as the F32 the arithmetic reads. An F16
/// genome widens exactly, so its shard evaluates as the reference does.
fn pack(
    parameters: Weights<'_>,
    topology: &DenseTopology,
    layers: &[Layer],
    out: &mut [f32],
) -> Result<(), CoreError> {
    match parameters {
        Weights::F32(values) => pack_as(values, topology, layers, out),
        Weights::F16(values) => pack_as(values, topology, layers, out),
    }
}

fn pack_as<W: Weight>(
    parameters: &[W],
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
                *slot = value.widen();
            }
        }
        for (slot, bias) in packed_biases.iter_mut().zip(biases) {
            *slot = bias.widen();
        }
    }
    Ok(())
}

/// Evaluate one organism's already-narrowed F32 inputs through its packed
/// layers directly into `out`, returning whether every activation and output
/// stayed finite (exact bit-for-bit parity with `ReferenceController`).
fn evaluate_f32_into(
    layers: &[Layer],
    values: &[f32],
    inputs: &[f32],
    out: &mut [f32],
    scratch: &mut Scratch,
) -> Result<bool, CoreError> {
    let Some((output, hidden)) = layers.split_last() else {
        return Ok(false);
    };
    let Scratch { first, second } = scratch;
    let activated = match hidden {
        [] => {
            output.apply(values, inputs, out)?;
            true
        }
        [one] => {
            let first = &mut first[..one.outputs];
            one.apply(values, inputs, first)?;
            activate(first) && {
                output.apply(values, first, out)?;
                true
            }
        }
        [one, two] => {
            let first = &mut first[..one.outputs];
            let second = &mut second[..two.outputs];
            one.apply(values, inputs, first)?;
            activate(first) && {
                two.apply(values, first, second)?;
                activate(second) && {
                    output.apply(values, second, out)?;
                    true
                }
            }
        }
        _ => false,
    };
    Ok(activated && out.iter().all(|value| value.is_finite()))
}

/// One organism's raw linear outputs from its packed slot, or `None` where the
/// reference would refuse it.
fn evaluate(
    layers: &[Layer],
    input_width: usize,
    output_width: usize,
    values: &[f32],
    inputs: &[f64],
    scratch: &mut Scratch,
) -> Result<Option<RawOutputs>, CoreError> {
    if inputs.len() != input_width
        || input_width > MAX_SENSOR_WIDTH
        || output_width == 0
        || output_width > MAX_CONTROLLER_OUTPUT_WIDTH
    {
        return Ok(None);
    }
    let mut rounded = [0.0_f32; MAX_SENSOR_WIDTH];
    let rounded = &mut rounded[..input_width];
    for (slot, &value) in rounded.iter_mut().zip(inputs) {
        // Rounding each input once to F32 is the declared arithmetic.
        *slot = value as f32;
        if !slot.is_finite() {
            return Ok(None);
        }
    }
    let mut raw = RawOutputs::zeroed(output_width)?;
    let ok = evaluate_f32_into(layers, values, rounded, raw.as_mut_slice(), scratch)?;
    Ok(ok.then_some(raw))
}

/// The reference's hidden activation in place: `tanh` of each F32
/// pre-activation in F64, rounded once to F32, as `tanh_f32` computes it.
/// False where it refuses one.
fn activate(values: &mut [f32]) -> bool {
    for value in values {
        let Some(activated) = tanh_f32(*value) else {
            return false;
        };
        *value = activated;
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

    /// Room for activations `width` wide, reserved to exact capacity so
    /// widening across topologies never overshoots [`BackendBudget::bytes`]
    /// through exponential `Vec` growth (FL-043).
    fn hold(&mut self, width: usize) -> Result<(), CoreError> {
        for buffer in [&mut self.first, &mut self.second] {
            if buffer.len() < width {
                let additional = width - buffer.len();
                buffer
                    .try_reserve_exact(additional)
                    .map_err(|_| CoreError::AllocationFailed {
                        buffer: "cpu controller activations",
                        bytes: u64::try_from(additional.saturating_mul(size_of::<f32>()))
                            .unwrap_or(u64::MAX),
                    })?;
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
    /// Live packed occupants in `occupants`; when deaths drop this to zero,
    /// the shard is reclaimed (FL-043).
    live_count: usize,
}

/// Per-topology shard map with O(1) direct indexing for normal arena shards
/// and a sparse `HashMap` fallback for arbitrary 64-bit shard IDs.
#[derive(Debug, Default)]
struct ShardTable {
    dense: Vec<Option<Shard>>,
    sparse: HashMap<u64, Shard>,
}

impl ShardTable {
    fn get(&self, key: u64) -> Option<&Shard> {
        if let Ok(index) = usize::try_from(key)
            && index < DENSE_SHARDS
        {
            return self.dense.get(index).and_then(Option::as_ref);
        }
        self.sparse.get(&key)
    }

    fn get_mut(&mut self, key: u64) -> Option<&mut Shard> {
        if let Ok(index) = usize::try_from(key)
            && index < DENSE_SHARDS
        {
            return self.dense.get_mut(index).and_then(Option::as_mut);
        }
        self.sparse.get_mut(&key)
    }

    fn insert(&mut self, key: u64, shard: Shard) {
        if let Ok(index) = usize::try_from(key)
            && index < DENSE_SHARDS
        {
            if self.dense.len() <= index {
                self.dense.resize_with(index + 1, || None);
            }
            self.dense[index] = Some(shard);
            return;
        }
        self.sparse.insert(key, shard);
    }

    fn remove(&mut self, key: u64) -> Option<Shard> {
        if let Ok(index) = usize::try_from(key)
            && index < DENSE_SHARDS
        {
            return self.dense.get_mut(index).and_then(Option::take);
        }
        self.sparse.remove(&key)
    }

    fn len(&self) -> usize {
        self.dense.iter().flatten().count() + self.sparse.len()
    }

    fn values(&self) -> impl Iterator<Item = &Shard> {
        self.dense.iter().flatten().chain(self.sparse.values())
    }
}

/// Division-free shard locator backed by a per-topology `[base, end)` range
/// cache, eliminating per-organism 64-bit integer divisions on large worlds.
#[inline]
fn locate_cached(
    geometry: &ShardGeometry,
    topology: usize,
    raw_slot: u64,
    cached_ranges: &mut [Option<(u64, u64, u64)>; TOPO_CACHE_SLOTS],
) -> (u64, u64, u64) {
    let slots_per_shard = geometry.slots_per_shard();
    let stride = geometry.slot_stride_elements();
    if raw_slot < slots_per_shard {
        (0_u64, raw_slot, raw_slot * stride)
    } else if topology < TOPO_CACHE_SLOTS
        && let Some((shard_key, base, end)) = cached_ranges[topology]
        && raw_slot >= base
        && raw_slot < end
    {
        let local = raw_slot - base;
        (shard_key, local, local * stride)
    } else {
        let at = geometry.locate(raw_slot);
        if topology < TOPO_CACHE_SLOTS
            && let Some(base) = at.shard.checked_mul(slots_per_shard)
            && let Some(end) = base.checked_add(slots_per_shard)
        {
            cached_ranges[topology] = Some((at.shard, base, end));
        }
        (at.shard, at.slot_in_shard, at.offset)
    }
}

/// Outcome of attempting to ensure one job's parameters are resident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaceOutcome {
    Resident,
    Packed,
    Deferred,
}

/// One catalog member's packed slots, in shards.
#[derive(Debug)]
struct Packing {
    input_width: usize,
    output_width: usize,
    layers: Vec<Layer>,
    widest: usize,
    geometry: ShardGeometry,
    shards: ShardTable,
}

impl Packing {
    /// The packed values of `(slot, genome)` at a pre-located shard/slot/offset.
    fn resident_located(
        &self,
        slot: ArenaSlot,
        genome: GenomeId,
        shard_key: u64,
        local_u64: u64,
        offset_u64: u64,
    ) -> Option<&[f32]> {
        let shard = self.shards.get(shard_key)?;
        let local = usize::try_from(local_u64).ok()?;
        let occupant = shard.occupants.get(local).copied().flatten()?;
        if occupant != (slot.generation(), genome) {
            return None;
        }
        let offset = usize::try_from(offset_u64).ok()?;
        let length = usize::try_from(self.geometry.parameters()).ok()?;
        shard.values.get(offset..offset.checked_add(length)?)
    }

    /// Pack `job`'s parameters into its pre-located slot, in place of any
    /// earlier occupant's, unless they are resident, or their shard is not made
    /// yet and would need more than `room` bytes (FL-043). Updates `held`
    /// atomically on shard creation or error-path reclamation so a packing
    /// failure never leaves a zombie shard or desyncs memory accounting.
    fn place_located(
        &mut self,
        job: &impl JobView,
        shard_key: u64,
        local_u64: u64,
        offset_u64: u64,
        room: u64,
        held: &mut u64,
    ) -> Result<PlaceOutcome, CoreError> {
        let slot = job.slot();
        let genome = job.genome();
        if self
            .resident_located(slot, genome, shard_key, local_u64, offset_u64)
            .is_some()
        {
            return Ok(PlaceOutcome::Resident);
        }
        let too_large = |quantity: &'static str| CoreError::Overflow { quantity };
        let local = usize::try_from(local_u64).map_err(|_| too_large("arena slot"))?;
        let offset = usize::try_from(offset_u64).map_err(|_| too_large("arena offset"))?;
        let length = usize::try_from(self.geometry.parameters())
            .map_err(|_| too_large("arena slot length"))?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| too_large("arena offset"))?;
        let shard_elements = usize::try_from(self.geometry.shard_elements())
            .map_err(|_| too_large("arena shard elements"))?;
        let slots = usize::try_from(self.geometry.slots_per_shard())
            .map_err(|_| too_large("arena shard slots"))?;
        let bytes = shard_bytes(&self.geometry);
        let controller = job.controller();
        let mut drop_empty_error = None;
        if let Some(shard) = self.shards.get_mut(shard_key) {
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
            if shard.occupants[local].take().is_some() {
                shard.live_count = shard.live_count.saturating_sub(1);
            }
            if let Err(error) = pack(
                controller.parameters(),
                controller.topology(),
                &self.layers,
                &mut shard.values[offset..end],
            ) {
                if shard.live_count == 0 {
                    drop_empty_error = Some(error);
                } else {
                    return Err(error);
                }
            } else {
                shard.occupants[local] = Some((slot.generation(), genome));
                shard.live_count += 1;
                return Ok(PlaceOutcome::Packed);
            }
        }
        if let Some(error) = drop_empty_error {
            self.shards.remove(shard_key);
            *held = held.saturating_sub(bytes);
            return Err(error);
        }
        if bytes > room {
            return Ok(PlaceOutcome::Deferred);
        }
        let mut shard = Shard::default();
        shard
            .values
            .try_reserve_exact(shard_elements)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "cpu controller shard",
                bytes: self.geometry.shard_elements().saturating_mul(4),
            })?;
        shard
            .occupants
            .try_reserve_exact(slots)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "cpu controller slots",
                bytes: bytes_for::<Option<(u32, GenomeId)>>(slots),
            })?;
        shard.values.resize(end, 0.0);
        shard.occupants.resize(local + 1, None);
        pack(
            controller.parameters(),
            controller.topology(),
            &self.layers,
            &mut shard.values[offset..end],
        )?;
        shard.occupants[local] = Some((slot.generation(), genome));
        shard.live_count = 1;
        self.shards.insert(shard_key, shard);
        *held = held.saturating_add(bytes);
        Ok(PlaceOutcome::Packed)
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
    /// Whether `job`'s topology and parameter count match this bucket's
    /// supported packing (checked on intra-batch pointer cache misses).
    fn matches_topology(&self, controller: ReferenceController<'_>) -> bool {
        let Some(packing) = self.packing.as_ref() else {
            return false;
        };
        let length = controller.parameters().len() as u64;
        *controller.topology() == self.topology && length == packing.geometry.parameters()
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

    /// Look up or initialize the bucket for `index`, validating topology and
    /// parameter length against `controller` on intra-batch cache misses.
    fn ensure_bucket<'a>(
        buckets: &'a mut Vec<Option<Bucket>>,
        shard_slots: u64,
        index: usize,
        controller: ReferenceController<'_>,
    ) -> Result<Option<&'a mut Bucket>, CoreError> {
        if buckets.len() <= index {
            let needed = index.checked_add(1).ok_or(CoreError::Overflow {
                quantity: "cpu controller buckets",
            })?;
            let more = needed - buckets.len();
            reserve(buckets, more, "cpu controller buckets")?;
            buckets.resize_with(needed, || None);
        }
        let topology = controller.topology();
        let length = controller.parameters().len();
        let length_u64 = length as u64;
        let bucket = buckets[index].get_or_insert_with(|| Bucket {
            topology: topology.clone(),
            packing: shape(topology, length).and_then(|layers| {
                let geometry =
                    ShardGeometry::fitting(length_u64, MAXIMUM_SHARD_ELEMENTS, shard_slots).ok()?;
                let widest = layers.split_last().map_or(0, |(_, hidden)| {
                    hidden.iter().map(|layer| layer.outputs).max().unwrap_or(0)
                });
                Some(Packing {
                    input_width: topology.input_width(),
                    output_width: topology.raw_output_width(),
                    layers,
                    widest,
                    geometry,
                    shards: ShardTable::default(),
                })
            }),
        });
        Ok(bucket.matches_topology(controller).then_some(bucket))
    }

    /// Pack every job's parameters that are not resident and whose shard
    /// fits the budget, and give the widest hidden layer among them.
    fn place_jobs<J: JobView>(
        &mut self,
        jobs: &[J],
        finite: Option<&[u8]>,
        expected_dims: Option<(usize, usize)>,
        input_len: impl Fn(&J) -> usize,
    ) -> Result<usize, CoreError> {
        let mut widest = 0;
        let lanes = self.lanes.get() as u64;
        let current_scratch_bytes = total(self.scratch.iter().map(Scratch::allocated_bytes));
        let mut cached_topos = TopoPointerCache::new();
        let mut cached_ranges: [Option<(u64, u64, u64)>; TOPO_CACHE_SLOTS] =
            [None; TOPO_CACHE_SLOTS];
        for (job_idx, job) in jobs.iter().enumerate() {
            if let Some(flags) = finite
                && flags[job_idx] == 0
            {
                continue;
            }
            let slot = job.slot();
            let Ok(index) = usize::try_from(slot.topology()) else {
                continue;
            };
            let controller = job.controller();
            let topo_ptr = std::ptr::from_ref(controller.topology());
            let length_u64 = controller.parameters().len() as u64;
            let Some(bucket) = (if cached_topos.contains(index, topo_ptr, length_u64) {
                self.buckets.get_mut(index).and_then(Option::as_mut)
            } else {
                let found =
                    Self::ensure_bucket(&mut self.buckets, self.shard_slots, index, controller)?;
                if found.is_some() {
                    cached_topos.insert(index, topo_ptr, length_u64);
                }
                found
            }) else {
                continue;
            };
            let Some(packing) = bucket.packing.as_mut() else {
                continue;
            };
            if let Some((in_w, out_w)) = expected_dims {
                if packing.input_width != in_w || packing.output_width != out_w {
                    continue;
                }
            } else if input_len(job) != packing.input_width {
                continue;
            }
            let (shard_key, local_u64, offset_u64) =
                locate_cached(&packing.geometry, index, slot.slot(), &mut cached_ranges);
            // The lanes' activations: two buffers of the widest layer served,
            // or what they already hold.
            let activations = lanes
                .saturating_mul(2)
                .saturating_mul(bytes_for::<f32>(widest.max(packing.widest)))
                .max(current_scratch_bytes);
            let room = self
                .budget
                .bytes
                .saturating_sub(self.held.saturating_add(activations));
            match packing.place_located(
                job,
                shard_key,
                local_u64,
                offset_u64,
                room,
                &mut self.held,
            )? {
                PlaceOutcome::Packed => {
                    self.packs += 1;
                    widest = widest.max(packing.widest);
                }
                PlaceOutcome::Resident => {
                    widest = widest.max(packing.widest);
                }
                PlaceOutcome::Deferred => {}
            }
        }
        Ok(widest)
    }

    fn prepare_lanes(&mut self, job_count: usize, widest: usize) -> Result<usize, CoreError> {
        let lanes = self
            .lanes
            .get()
            .min(job_count.div_ceil(MINIMUM_JOBS_PER_LANE))
            .max(1);
        if let Some(missing) = lanes.checked_sub(self.scratch.len()).filter(|&n| n > 0) {
            reserve(&mut self.scratch, missing, "cpu controller lanes")?;
            self.scratch.resize_with(lanes, Scratch::default);
        }
        for scratch in &mut self.scratch[..lanes] {
            scratch.hold(widest)?;
        }
        Ok(lanes)
    }

    fn evaluate_flat_f32_inner(
        &mut self,
        jobs: &[FlatControllerJob<'_>],
        input_width: usize,
        inputs_f32: &[f32],
        inputs_finite: &[u8],
        outputs: &mut FlatOutputs,
    ) -> Result<(), CoreError> {
        self.used = 0;
        validate_flat_inputs(
            jobs.len(),
            input_width,
            inputs_f32.len(),
            inputs_finite.len(),
        )?;
        let Some(first) = jobs.first() else {
            outputs.clear();
            return Ok(());
        };
        let output_width = jobs
            .iter()
            .find_map(|job| {
                let topo = job.controller.topology();
                (topo.input_width() == input_width).then_some(topo.raw_output_width())
            })
            .unwrap_or_else(|| first.controller.topology().raw_output_width());
        outputs.prepare(jobs.len(), output_width)?;
        if input_width == 0
            || input_width > MAX_SENSOR_WIDTH
            || output_width == 0
            || output_width > MAX_CONTROLLER_OUTPUT_WIDTH
        {
            return Ok(());
        }
        let widest = self.place_jobs(
            jobs,
            Some(inputs_finite),
            Some((input_width, output_width)),
            |_| input_width,
        )?;
        let lanes = self.prepare_lanes(jobs.len(), widest)?;
        let buckets = &self.buckets;
        let length = jobs.len().div_ceil(lanes);
        let in_chunk = length.checked_mul(input_width).ok_or(CoreError::Overflow {
            quantity: "flat controller input chunk",
        })?;
        let out_chunk = length
            .checked_mul(output_width)
            .ok_or(CoreError::Overflow {
                quantity: "flat controller output chunk",
            })?;
        let mut runs = jobs
            .chunks(length)
            .zip(inputs_f32.chunks(in_chunk))
            .zip(inputs_finite.chunks(length))
            .zip(outputs.values.chunks_mut(out_chunk))
            .zip(outputs.valid.chunks_mut(length))
            .zip(self.scratch.iter_mut())
            .map(
                |(((((chunk_jobs, chunk_in), chunk_fin), chunk_out), chunk_valid), scratch)| {
                    (
                        FlatChunk {
                            jobs: chunk_jobs,
                            input_width,
                            output_width,
                            inputs_f32: chunk_in,
                            inputs_finite: chunk_fin,
                            out_values: chunk_out,
                            out_valid: chunk_valid,
                        },
                        scratch,
                    )
                },
            );
        let Some((head_chunk, head_scratch)) = runs.next() else {
            return Ok(());
        };
        if lanes == 1 {
            self.used = 1;
            return run_flat_f32(buckets, head_chunk, head_scratch);
        }
        let tail_count = runs.len();
        let used = 1 + tail_count;
        let mut tail_results: Vec<Result<(), CoreError>> =
            (0..tail_count).map(|_| Ok(())).collect();
        let scoped = pool::scope_workers(self.lanes.get(), "cpu controller lanes", |scope| {
            for ((chunk, scratch), slot) in runs.zip(tail_results.iter_mut()) {
                scope.spawn(move || {
                    *slot = run_flat_f32(buckets, chunk, scratch);
                });
            }
            run_flat_f32(buckets, head_chunk, head_scratch)
        });
        self.used = used;
        let mut result = match scoped {
            Ok(head_result) => head_result,
            Err(error) => Err(error),
        };
        for tail_result in tail_results {
            if result.is_ok() {
                result = tail_result;
            }
        }
        result
    }
}

/// Safe intra-batch per-topology pointer cache: every `&'a DenseTopology`
/// borrowed inside a batch stays alive and immutable for the whole call,
/// eliminating per-organism `DenseTopology` `Vec` equality comparisons.
struct TopoPointerCache {
    slots: [Option<(*const DenseTopology, u64)>; TOPO_CACHE_SLOTS],
    overflow: Option<(usize, *const DenseTopology, u64)>,
}

impl TopoPointerCache {
    const fn new() -> Self {
        Self {
            slots: [None; TOPO_CACHE_SLOTS],
            overflow: None,
        }
    }

    #[inline]
    fn contains(&self, index: usize, ptr: *const DenseTopology, len: u64) -> bool {
        if index < TOPO_CACHE_SLOTS {
            matches!(
                self.slots[index],
                Some((cached_ptr, cached_len))
                    if std::ptr::eq(cached_ptr, ptr) && cached_len == len
            )
        } else {
            matches!(
                self.overflow,
                Some((cached_idx, cached_ptr, cached_len))
                    if cached_idx == index && std::ptr::eq(cached_ptr, ptr) && cached_len == len
            )
        }
    }

    #[inline]
    fn insert(&mut self, index: usize, ptr: *const DenseTopology, len: u64) {
        if index < TOPO_CACHE_SLOTS {
            self.slots[index] = Some((ptr, len));
        } else {
            self.overflow = Some((index, ptr, len));
        }
    }
}

fn validate_flat_inputs(
    jobs_len: usize,
    input_width: usize,
    inputs_f32_len: usize,
    inputs_finite_len: usize,
) -> Result<(), CoreError> {
    let expected_inputs = jobs_len
        .checked_mul(input_width)
        .ok_or(CoreError::Overflow {
            quantity: "flat controller inputs",
        })?;
    if inputs_f32_len != expected_inputs {
        return Err(CoreError::LayoutMismatch {
            buffer: "flat controller inputs",
            detail: format!(
                "expected {expected_inputs} f32 inputs ({jobs_len} jobs x {input_width}), got {inputs_f32_len}"
            ),
        });
    }
    if inputs_finite_len != jobs_len {
        return Err(CoreError::LayoutMismatch {
            buffer: "flat controller input flags",
            detail: format!("expected {jobs_len} finite flags, got {inputs_finite_len}"),
        });
    }
    Ok(())
}

#[inline]
fn packing_cached<'a>(
    buckets: &'a [Option<Bucket>],
    index: usize,
    controller: ReferenceController<'_>,
    cache: &mut TopoPointerCache,
) -> Option<&'a Packing> {
    let bucket = buckets.get(index).and_then(Option::as_ref)?;
    let topo_ptr = std::ptr::from_ref(controller.topology());
    let len_u64 = controller.parameters().len() as u64;
    if !cache.contains(index, topo_ptr, len_u64) {
        if !bucket.matches_topology(controller) {
            return None;
        }
        cache.insert(index, topo_ptr, len_u64);
    }
    bucket.packing.as_ref()
}

/// Evaluate a run of legacy jobs into its outputs.
fn run(
    buckets: &[Option<Bucket>],
    jobs: &[ControllerJob<'_>],
    outputs: &mut [Option<RawOutputs>],
    scratch: &mut Scratch,
) -> Result<(), CoreError> {
    let mut cached_topos = TopoPointerCache::new();
    let mut cached_ranges: [Option<(u64, u64, u64)>; TOPO_CACHE_SLOTS] = [None; TOPO_CACHE_SLOTS];
    for (job, output) in jobs.iter().zip(outputs) {
        let Ok(index) = usize::try_from(job.slot.topology()) else {
            *output = None;
            continue;
        };
        let Some(packing) = packing_cached(buckets, index, job.controller, &mut cached_topos)
        else {
            *output = None;
            continue;
        };
        if job.inputs.len() != packing.input_width {
            *output = None;
            continue;
        }
        let (shard_key, local_u64, offset_u64) = locate_cached(
            &packing.geometry,
            index,
            job.slot.slot(),
            &mut cached_ranges,
        );
        *output = match packing
            .resident_located(job.slot, job.genome, shard_key, local_u64, offset_u64)
        {
            Some(values) => evaluate(
                &packing.layers,
                packing.input_width,
                packing.output_width,
                values,
                job.inputs,
                scratch,
            )?,
            None => None,
        };
    }
    Ok(())
}

/// Split flat-F32 slices owned by one CPU worker lane.
struct FlatChunk<'a, 'j> {
    jobs: &'a [FlatControllerJob<'j>],
    input_width: usize,
    output_width: usize,
    inputs_f32: &'a [f32],
    inputs_finite: &'a [u8],
    out_values: &'a mut [f32],
    out_valid: &'a mut [u8],
}

/// Evaluate a contiguous slice of flat-F32 jobs directly from `inputs_f32` into
/// `out_values` + `out_valid` with zero `f64 -> f32` narrowing and zero
/// `Option<RawOutputs>` allocation/copy.
fn run_flat_f32(
    buckets: &[Option<Bucket>],
    chunk: FlatChunk<'_, '_>,
    scratch: &mut Scratch,
) -> Result<(), CoreError> {
    let FlatChunk {
        jobs,
        input_width,
        output_width,
        inputs_f32,
        inputs_finite,
        out_values,
        out_valid,
    } = chunk;
    let mut cached_topos = TopoPointerCache::new();
    let mut cached_ranges: [Option<(u64, u64, u64)>; TOPO_CACHE_SLOTS] = [None; TOPO_CACHE_SLOTS];
    for ((((job, input_row), &finite), out_row), valid) in jobs
        .iter()
        .zip(inputs_f32.chunks_exact(input_width))
        .zip(inputs_finite)
        .zip(out_values.chunks_exact_mut(output_width))
        .zip(out_valid)
    {
        if finite == 0 {
            *valid = 0;
            continue;
        }
        let Ok(index) = usize::try_from(job.slot.topology()) else {
            *valid = 0;
            continue;
        };
        let Some(packing) = packing_cached(buckets, index, job.controller, &mut cached_topos)
        else {
            *valid = 0;
            continue;
        };
        if packing.input_width != input_width || packing.output_width != output_width {
            *valid = 0;
            continue;
        }
        let (shard_key, local_u64, offset_u64) = locate_cached(
            &packing.geometry,
            index,
            job.slot.slot(),
            &mut cached_ranges,
        );
        let Some(values) =
            packing.resident_located(job.slot, job.genome, shard_key, local_u64, offset_u64)
        else {
            *valid = 0;
            continue;
        };
        let ok = evaluate_f32_into(&packing.layers, values, input_row, out_row, scratch)?;
        *valid = u8::from(ok);
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

    fn release_slots(&mut self, slots: &[ArenaSlot]) {
        for &slot in slots {
            let Ok(topology) = usize::try_from(slot.topology()) else {
                continue;
            };
            let Some(packing) = self
                .buckets
                .get_mut(topology)
                .and_then(Option::as_mut)
                .and_then(|bucket| bucket.packing.as_mut())
            else {
                continue;
            };
            let at = packing.geometry.locate(slot.slot());
            let Ok(local) = usize::try_from(at.slot_in_shard) else {
                continue;
            };
            let drop_shard = match packing.shards.get_mut(at.shard) {
                Some(shard) => {
                    if let Some(occupant) = shard.occupants.get_mut(local)
                        && occupant.is_some_and(|(generation, _)| generation == slot.generation())
                    {
                        *occupant = None;
                        shard.live_count = shard.live_count.saturating_sub(1);
                    }
                    shard.live_count == 0
                }
                None => false,
            };
            if drop_shard && packing.shards.remove(at.shard).is_some() {
                self.held = self.held.saturating_sub(shard_bytes(&packing.geometry));
            }
        }
    }

    fn supports_flat_f32(&self) -> bool {
        true
    }

    fn evaluate_flat_f32(
        &mut self,
        jobs: &[FlatControllerJob<'_>],
        input_width: usize,
        inputs_f32: &[f32],
        inputs_finite: &[u8],
        outputs: &mut FlatOutputs,
    ) -> Result<(), CoreError> {
        let result =
            self.evaluate_flat_f32_inner(jobs, input_width, inputs_f32, inputs_finite, outputs);
        if result.is_err() {
            outputs.clear();
        }
        result
    }

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<RawOutputs>>,
    ) -> Result<(), CoreError> {
        self.used = 0;
        if jobs.is_empty() {
            outputs.clear();
            return Ok(());
        }
        if outputs.len() > jobs.len() {
            outputs.truncate(jobs.len());
        } else if outputs.len() < jobs.len() {
            if outputs.capacity() < jobs.len()
                && let Err(error) = reserve(
                    outputs,
                    jobs.len() - outputs.len(),
                    "cpu controller outputs",
                )
            {
                outputs.clear();
                return Err(error);
            }
            outputs.resize(jobs.len(), None);
        }
        let widest = match self.place_jobs(jobs, None, None, |job| job.inputs.len()) {
            Ok(widest) => widest,
            Err(error) => {
                outputs.clear();
                return Err(error);
            }
        };
        let lanes = match self.prepare_lanes(jobs.len(), widest) {
            Ok(lanes) => lanes,
            Err(error) => {
                outputs.clear();
                return Err(error);
            }
        };
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
            let result = run(buckets, head_jobs, head_outputs, head_scratch);
            if result.is_err() {
                outputs.clear();
            }
            return result;
        }
        let tail_count = runs.len();
        let used = 1 + tail_count;
        let mut tail_results: Vec<Result<(), CoreError>> =
            (0..tail_count).map(|_| Ok(())).collect();
        // Pass `self.lanes.get()` (the stable configured worker budget) so
        // `CpuBackend` shares the exact same cached private pool (`flife-w{W}-*`)
        // as every other phase without touching Rayon's global pool.
        let scoped = pool::scope_workers(self.lanes.get(), "cpu controller lanes", |scope| {
            for (((chunk_jobs, chunk_outputs), chunk_scratch), slot) in
                runs.zip(tail_results.iter_mut())
            {
                scope.spawn(move || {
                    *slot = run(buckets, chunk_jobs, chunk_outputs, chunk_scratch);
                });
            }
            run(buckets, head_jobs, head_outputs, head_scratch)
        });
        self.used = used;
        let mut result = match scoped {
            Ok(head_result) => head_result,
            Err(error) => Err(error),
        };
        for tail_result in tail_results {
            if result.is_ok() {
                result = tail_result;
            }
        }
        if result.is_err() {
            outputs.clear();
        }
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
