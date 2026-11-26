//! FrankenLife's Metal backend (plan §10.2, FL-030): a batch of organisms'
//! own dense controllers evaluated in one fused dispatch, one threadgroup
//! per organism, through FrankenTorch's generic Metal compute gateway.
//!
//! The kernel reproduces [`flife_neural::ReferenceController::evaluate`] bit
//! for bit, not within a tolerance. Each neuron accumulates in F32 in the
//! declared order (bias, then the inputs ascending, every product and sum
//! rounded, no fused multiply-add), and the hidden activation is
//! `flife_core::numerics::tanh` run on a software binary64 (Apple GPUs have
//! no F64), rounded once to F32 as the reference rounds it. Its fast path
//! computes the activation in float-float on the hardware's F32 and keeps
//! the result only where every value within 2^-38 of it rounds to the same
//! F32; elsewhere it falls back to the software binary64. The gateway
//! compiles the kernel in its IEEE-safe math mode.
//!
//! The device flushes subnormal F32 results to zero, and the reference does
//! not. The kernel defers to the caller any organism whose arithmetic comes
//! near the subnormal range (a nonzero operand or product below 2^-102,
//! which every step that could produce a subnormal has), and any whose
//! pre-activation or output is not finite. The caller evaluates a deferred
//! organism on the reference controller, for its exact value or its error.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::num::NonZeroUsize;
use std::sync::OnceLock;

use flife_backend_cpu::CpuBackend;
use flife_core::ids::GenomeId;
use flife_core::memory::{bytes_for, capacity_bytes, total};
use flife_core::{CoreError, pool};
use flife_neural::{
    ArenaSlot, ControllerBackend, ControllerJob, DenseTopology, FlatControllerJob, FlatOutputs,
    MAX_CONTROLLER_OUTPUT_WIDTH, MAX_SENSOR_WIDTH, RawOutputs, ReferenceController, ShardGeometry,
    WeightDtype, Weights,
};
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

pub mod birth;
pub mod check;
pub mod chemistry;
pub mod emission;
pub mod field;
pub mod motion;
pub mod owner;
pub mod proposal;
pub mod reduce;
pub mod sensors;
pub mod settle;
pub mod uptake;

pub use birth::{BIRTH_OPS_WORDS, MetalBirth, ParameterBits};
pub use chemistry::{MetalChemistry, SETTLE_OPS_WORDS};
pub use emission::{EMISSION_OPS_WORDS, MetalEmission};
pub use field::MetalField;
/// Phases 6 to 8's plain data (FL-032), which [`MetalChemistry`],
/// [`MetalMotion`], [`MetalEmission`], [`MetalProposal`] and [`MetalBirth`]
/// take and give: flife-field's port types.
pub use flife_field::settle::{
    ChemistryConstants, ChemistryOrganism, ChemistryOutcome, ChemistryPacket, EmissionAction,
    EmissionOrganism, EmissionOutcome, ExhaustedAt, MOTION_TERMS, MUTATION_LAYERS,
    MaintenanceOutcome, MotionConstants, MotionOrganism, MotionOutcome, MotionV2Extras,
    MotorSteeringConstants, MotorSteeringInputs, MovementOutcome, MutationBirth, MutationConstants,
    MutationCounts, MutationLayer, MutationValues, ProposalConstants, ProposalOrganism,
    ProposalOutcome,
};
/// The memory a controller backend may hold (FL-043), for composition
/// roots that make one through [`ControllerChoice::backend`].
pub use flife_neural::BackendBudget;
pub use motion::{MOTION_OPS_WORDS, MetalMotion};
use owner::metal;
pub use owner::{
    Binding, Buffer, DeviceOwner, DeviceTime, Element, Kernel, Limits, Owned, Purpose, ReadLease,
    Receipt, ResourceState, Usage, WriteLease,
};
pub use proposal::{MetalProposal, PROPOSAL_OPS_WORDS};
pub use sensors::MetalSensors;
pub use settle::MetalSettle;
pub use uptake::MetalUptake;

/// The controller kernel source, with the software binary64
/// (`softfloat64.metal`, shared with [`field::FIELD_MSL`]) ahead of it.
pub const CONTROLLER_MSL: &str = concat!(
    include_str!("softfloat64.metal"),
    include_str!("controller.metal")
);

/// The widest hidden layer the kernel's threadgroup storage holds.
pub const WIDEST_HIDDEN: usize = 128;

/// Threads per organism in the forward dispatch for wide topologies
/// (`widest_hidden > 64`).
pub const FORWARD_THREADS: usize = 64;

/// Single-SIMD-group threads per organism for compact topologies
/// (`widest_hidden <= 64`, including `[8]`, `[32]`, and the dominant `[64, 32]`):
/// eliminates idle second SIMD groups on 32/36-wide layers and uses
/// `simdgroup_barrier` + register `simd_or` without threadgroup atomics.
const FORWARD_THREADS_COMPACT: usize = 32;

/// Choose the adaptive threadgroup size for a topology whose widest hidden
/// layer has `widest_hidden` neurons.
const fn forward_threads(widest_hidden: usize) -> usize {
    if widest_hidden <= 64 {
        FORWARD_THREADS_COMPACT
    } else {
        FORWARD_THREADS
    }
}

/// The jobs words per organism: its row, parameter offset, hidden-layer
/// count and two hidden widths.
const JOB_WORDS: usize = 5;

/// Words per row of [`MetalControllers::binary64_ops`]'s results.
pub const BINARY64_RESULT_WORDS: usize = 12;

/// A batch of controller evaluations, packed for the kernel: every
/// organism's parameters back to back, its inputs rounded once to F32 as the
/// reference rounds them, its topology, and the batch's shared
/// `(input_width, raw_output_width)` dimensions.
#[derive(Debug, Default, Clone)]
pub struct Batch {
    dims: Option<(usize, usize)>,
    parameters: Vec<f32>,
    inputs: Vec<f32>,
    jobs: Vec<u32>,
}

impl Batch {
    /// An empty batch.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            dims: None,
            parameters: Vec::new(),
            inputs: Vec::new(),
            jobs: Vec::new(),
        }
    }

    /// Organisms in the batch.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.jobs.len() / JOB_WORDS
    }

    /// Whether the batch is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// Empty the batch, keeping its storage.
    pub fn clear(&mut self) {
        self.dims = None;
        self.parameters.clear();
        self.inputs.clear();
        self.jobs.clear();
    }

    /// Add one organism: its controller's topology and parameters, and its
    /// inputs (length `topology.input_width()`, up to [`MAX_SENSOR_WIDTH`]).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for an input that is not finite in
    /// F32, exactly as the reference refuses it;
    /// [`CoreError::LayoutMismatch`] for parameters that do not fill the
    /// topology, mismatched input length or batch dimensions, or a topology
    /// outside the kernel's shape (one or two hidden layers, each at most
    /// [`WIDEST_HIDDEN`] wide, inputs `<= 128`, outputs `<= 80`);
    /// [`CoreError::AllocationFailed`] if the batch cannot grow. The batch is
    /// unchanged on error.
    pub fn push(
        &mut self,
        topology: &DenseTopology,
        parameters: &[f32],
        inputs: &[f64],
    ) -> Result<(), CoreError> {
        let input_width = topology.input_width();
        let output_width = topology.raw_output_width();
        if inputs.len() != input_width {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal controller batch",
                detail: format!("expected {input_width} inputs, got {}", inputs.len()),
            });
        }
        if let Some((batch_in, batch_out)) = self.dims
            && (batch_in, batch_out) != (input_width, output_width)
        {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal controller batch",
                detail: format!(
                    "batch dims ({batch_in}, {batch_out}) do not match topology \
                     ({input_width}, {output_width})"
                ),
            });
        }
        let job = job_words(
            topology,
            parameters.len(),
            self.parameters.len(),
            self.len(),
        )?;
        let next_organisms =
            self.len()
                .checked_add(1)
                .ok_or_else(|| CoreError::LayoutMismatch {
                    buffer: "metal controller batch",
                    detail: "organism count overflows".to_owned(),
                })?;
        let in_end =
            next_organisms
                .checked_mul(input_width)
                .ok_or_else(|| CoreError::LayoutMismatch {
                    buffer: "metal controller batch",
                    detail: "input staging count overflows".to_owned(),
                })?;
        let out_end =
            next_organisms
                .checked_mul(output_width)
                .ok_or_else(|| CoreError::LayoutMismatch {
                    buffer: "metal controller batch",
                    detail: "output staging count overflows".to_owned(),
                })?;
        if u32::try_from(in_end).is_err() || u32::try_from(out_end).is_err() {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal controller batch",
                detail: "staging index exceeds u32".to_owned(),
            });
        }
        let mut rounded = [0.0_f32; MAX_SENSOR_WIDTH];
        let rounded = &mut rounded[..input_width];
        for (index, (slot, &value)) in rounded.iter_mut().zip(inputs).enumerate() {
            let narrowed = value as f32;
            if !narrowed.is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "controller input",
                    detail: format!("input {index} is {value}, not finite in F32"),
                });
            }
            *slot = narrowed;
        }
        reserve(&mut self.parameters, parameters.len(), "batch parameters")?;
        reserve(&mut self.inputs, input_width, "batch inputs")?;
        reserve(&mut self.jobs, JOB_WORDS, "batch jobs")?;
        self.dims = Some((input_width, output_width));
        self.parameters.extend_from_slice(parameters);
        self.inputs.extend_from_slice(rounded);
        self.jobs.extend_from_slice(&job);
        Ok(())
    }
}

/// The kernel's job words for `topology` at `offset`, its organism at
/// `row`, after checking that its layers sit back to back from zero as the
/// kernel walks them and that every word fits 32 bits.
fn job_words(
    topology: &DenseTopology,
    length: usize,
    offset: usize,
    row: usize,
) -> Result<[u32; JOB_WORDS], CoreError> {
    let mismatch = |detail: String| CoreError::LayoutMismatch {
        buffer: "metal controller batch",
        detail,
    };
    let input_width = topology.input_width();
    let output_width = topology.raw_output_width();
    if input_width == 0
        || input_width > MAX_SENSOR_WIDTH
        || output_width == 0
        || output_width > MAX_CONTROLLER_OUTPUT_WIDTH
    {
        return Err(mismatch(format!(
            "controller dims ({input_width}, {output_width}) outside (1..={MAX_SENSOR_WIDTH}, \
             1..={MAX_CONTROLLER_OUTPUT_WIDTH})"
        )));
    }
    if length != topology.parameter_count() {
        return Err(mismatch(format!(
            "{length} parameters for a topology of {}",
            topology.parameter_count()
        )));
    }
    let widths = topology.hidden_widths();
    if widths.is_empty()
        || widths.len() > 2
        || widths.iter().any(|&w| w == 0 || w as usize > WIDEST_HIDDEN)
    {
        return Err(mismatch(format!("hidden widths {widths:?}")));
    }
    let mut at = 0;
    let mut fan_in = input_width;
    for (index, layer) in topology.layers().iter().enumerate() {
        let fan_out = widths
            .get(index)
            .map_or(output_width, |&width| width as usize);
        if layer.inputs != fan_in
            || layer.outputs != fan_out
            || layer.weights != at
            || layer.biases != at + fan_in * fan_out
        {
            return Err(mismatch(format!("layer {index} is {layer:?}")));
        }
        at = layer.biases + fan_out;
        fan_in = fan_out;
    }
    if topology.layers().len() != widths.len() + 1 || at != length {
        return Err(mismatch(format!("{} layers", topology.layers().len())));
    }
    let word = |value: usize, what: &str| {
        u32::try_from(value).map_err(|_| mismatch(format!("{what} {value} exceeds u32")))
    };
    let end = offset
        .checked_add(length)
        .ok_or_else(|| mismatch(format!("parameter offset {offset} overflows")))?;
    word(end, "parameter end")?;
    Ok([
        word(row, "row")?,
        word(offset, "parameter offset")?,
        word(widths.len(), "hidden layers")?,
        widths.first().copied().unwrap_or(0),
        widths.get(1).copied().unwrap_or(0),
    ])
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

/// A batch on the device: its parameters, inputs, jobs, dimensions, and room
/// for its outputs. Its buffers are retired when it is dropped.
pub struct Resident {
    organisms: usize,
    output_width: usize,
    parameters: Owned<f32>,
    inputs: Owned<f32>,
    jobs: Owned<u32>,
    raw: Owned<f32>,
    faults: Owned<u32>,
    dims: Owned<u32>,
}

impl std::fmt::Debug for Resident {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Resident")
            .field("organisms", &self.organisms)
            .field("parameters", &self.parameters.buffer())
            .finish_non_exhaustive()
    }
}

/// The compiled controller kernels on this host's Metal device, and the
/// owner of every buffer they use (FL-028).
pub struct MetalControllers {
    owner: DeviceOwner,
    forward: Pipeline,
    /// The forward pass over binary16 shards (FL-030).
    forward_half: Pipeline,
    activation: Pipeline,
    binary64: Pipeline,
    binary32: Pipeline,
}

impl std::fmt::Debug for MetalControllers {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalControllers")
            .field("device", &self.owner.device_name())
            .finish_non_exhaustive()
    }
}

impl MetalControllers {
    /// Open the default Metal device and compile the kernels.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] without a Metal device (always, off
    /// macOS); [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn open() -> Result<Self, CoreError> {
        Self::open_with(Limits::UNBOUNDED)
    }

    /// As [`MetalControllers::open`], its owner holding at most `limits`.
    ///
    /// # Errors
    /// As [`MetalControllers::open`].
    pub fn open_with(limits: Limits) -> Result<Self, CoreError> {
        Self::with_owner(DeviceOwner::open(limits)?)
    }

    /// Compile the controller kernels on an existing [`DeviceOwner`] so all
    /// phases of a world can share one device owner, VRAM budget, and device
    /// timer.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn with_owner(owner: DeviceOwner) -> Result<Self, CoreError> {
        let library = owner.library(CONTROLLER_MSL, MathMode::Safe)?;
        let pipeline = |name: &str| library.pipeline(name).map_err(metal);
        Ok(Self {
            forward: pipeline("controller_forward")?,
            forward_half: pipeline("controller_forward_half")?,
            activation: pipeline("activation_bits")?,
            binary64: pipeline("binary64_ops")?,
            binary32: pipeline("binary32_ops")?,
            owner,
        })
    }

    /// The device's name.
    #[must_use]
    pub fn device_name(&self) -> String {
        self.owner.device_name()
    }

    /// The owner of every buffer these kernels use.
    #[must_use]
    pub const fn owner(&self) -> &DeviceOwner {
        &self.owner
    }

    /// Copy `batch` to the device, for [`Self::run`].
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for an empty batch;
    /// [`CoreError::AllocationFailed`] if the device refuses a buffer.
    pub fn upload(&self, batch: &Batch) -> Result<Resident, CoreError> {
        let organisms = batch.len();
        let Some((input_width, output_width)) = batch.dims else {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal controller batch",
                detail: "an empty batch".to_owned(),
            });
        };
        if organisms == 0 {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal controller batch",
                detail: "an empty batch".to_owned(),
            });
        }
        let dims = [
            u32::try_from(input_width).map_err(|_| CoreError::Overflow {
                quantity: "metal controller input width",
            })?,
            u32::try_from(output_width).map_err(|_| CoreError::Overflow {
                quantity: "metal controller output width",
            })?,
        ];
        Ok(Resident {
            organisms,
            output_width,
            parameters: self.owner.upload_owned(&batch.parameters)?,
            inputs: self.owner.upload_owned(&batch.inputs)?,
            jobs: self.owner.upload_owned(&batch.jobs)?,
            raw: self.owner.zeroed_owned(organisms * output_width)?,
            faults: self.owner.zeroed_owned(organisms)?,
            dims: self.owner.upload_owned(&dims)?,
        })
    }

    /// Evaluate every organism of an uploaded batch, in its order. Each
    /// entry is the organism's raw linear outputs (length `raw_output_width`),
    /// bit-identical to `ReferenceController::evaluate_raw`, or `None` for a
    /// deferred organism (see the crate documentation): evaluate that one on
    /// the reference controller.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if the dispatch fails;
    /// [`CoreError::AllocationFailed`] if `outputs` cannot grow.
    pub fn run(
        &self,
        resident: &Resident,
        outputs: &mut Vec<Option<RawOutputs>>,
    ) -> Result<(), CoreError> {
        outputs.clear();
        let organisms = resident.organisms;
        let output_width = resident.output_width;
        reserve(outputs, organisms, "metal controller outputs")?;
        self.owner.submit(
            &self.forward,
            &[
                resident.parameters.buffer().reads(),
                resident.inputs.buffer().reads(),
                resident.jobs.buffer().reads(),
                resident.raw.buffer().writes(),
                resident.faults.buffer().writes(),
                resident.dims.buffer().reads(),
            ],
            Grid::linear(organisms, FORWARD_THREADS),
        )?;
        let mut values = vec![0.0_f32; organisms * output_width];
        self.owner.read_back(resident.raw.buffer(), &mut values)?;
        let mut flags = vec![0_u32; organisms];
        self.owner.read_back(resident.faults.buffer(), &mut flags)?;
        for (row, &flag) in values.chunks_exact(output_width).zip(&flags) {
            outputs.push(if flag == 0 {
                Some(RawOutputs::from_slice(row)?)
            } else {
                None
            });
        }
        Ok(())
    }

    /// [`Self::upload`] then [`Self::run`]; an empty batch gives no outputs.
    ///
    /// # Errors
    /// As those two.
    pub fn evaluate(
        &self,
        batch: &Batch,
        outputs: &mut Vec<Option<RawOutputs>>,
    ) -> Result<(), CoreError> {
        if batch.is_empty() {
            outputs.clear();
            return Ok(());
        }
        let resident = self.upload(batch)?;
        self.run(&resident, outputs)
    }

    /// The kernel's activation alone, for `count` consecutive F32 bit
    /// patterns from `base`: each result is the F32 bits of
    /// `numerics::tanh(f64::from(z)) as f32` as the kernel computes it, by
    /// the forward pass's activation or, with `software_only`, by the
    /// software binary64 alone. Returns how many of the forward pass's
    /// evaluations fell back to the software binary64. `count` must be a
    /// positive multiple of 256.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for another count, or one running past
    /// `u32::MAX`; [`CoreError::StateConflict`] if the dispatch fails.
    pub fn activation_bits(
        &self,
        base: u32,
        count: usize,
        software_only: bool,
        out: &mut [u32],
    ) -> Result<u32, CoreError> {
        let end = u64::from(base) + count as u64;
        if count == 0 || !count.is_multiple_of(256) || out.len() != count || end > 1 << 32 {
            return Err(CoreError::LayoutMismatch {
                buffer: "activation bits",
                detail: format!("{count} patterns from {base:#x} into {}", out.len()),
            });
        }
        let start = self.owner.upload_owned(&[base, u32::from(software_only)])?;
        let results = self.owner.zeroed_owned::<u32>(count)?;
        let fallbacks = self.owner.zeroed_owned::<u32>(1)?;
        self.owner.submit(
            &self.activation,
            &[
                start.buffer().reads(),
                results.buffer().writes(),
                fallbacks.buffer().writes(),
            ],
            Grid::linear(count / 256, 256),
        )?;
        self.owner.read_back(results.buffer(), out)?;
        let mut fell = [0_u32];
        self.owner.read_back(fallbacks.buffer(), &mut fell)?;
        Ok(fell[0])
    }

    /// The kernel's software binary64 on operand pairs `(a, b)` as bit
    /// patterns. Each result row is `add, sub, mul, div, round(a)` as
    /// `(low, high)` word pairs, then the F32 bits of `a as f32` and `b as
    /// f32`. `pairs.len()` must be a positive multiple of 64.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for another length;
    /// [`CoreError::StateConflict`] if the dispatch fails.
    pub fn binary64_ops(
        &self,
        pairs: &[[u64; 2]],
        out: &mut Vec<[u32; BINARY64_RESULT_WORDS]>,
    ) -> Result<(), CoreError> {
        if pairs.is_empty() || !pairs.len().is_multiple_of(64) {
            return Err(CoreError::LayoutMismatch {
                buffer: "binary64 operands",
                detail: format!("{} pairs", pairs.len()),
            });
        }
        let words: Vec<u32> = pairs
            .iter()
            .flat_map(|pair| pair.iter().flat_map(|&v| [v as u32, (v >> 32) as u32]))
            .collect();
        let operands = self.owner.upload_owned(&words)?;
        let results = self
            .owner
            .zeroed_owned::<u32>(pairs.len() * BINARY64_RESULT_WORDS)?;
        self.owner.submit(
            &self.binary64,
            &[operands.buffer().reads(), results.buffer().writes()],
            Grid::linear(pairs.len() / 64, 64),
        )?;
        let mut flat = vec![0_u32; pairs.len() * BINARY64_RESULT_WORDS];
        self.owner.read_back(results.buffer(), &mut flat)?;
        out.clear();
        out.extend(flat.as_chunks::<BINARY64_RESULT_WORDS>().0.iter().copied());
        Ok(())
    }

    /// The device's F32 product, sum and `fma(a, b, -(a * b))` of each pair,
    /// as the kernel computes them. `pairs.len()` must be a positive multiple
    /// of 64.
    ///
    /// # Errors
    /// As [`Self::binary64_ops`].
    pub fn binary32_ops(
        &self,
        pairs: &[[f32; 2]],
        out: &mut Vec<[f32; 3]>,
    ) -> Result<(), CoreError> {
        if pairs.is_empty() || !pairs.len().is_multiple_of(64) {
            return Err(CoreError::LayoutMismatch {
                buffer: "binary32 operands",
                detail: format!("{} pairs", pairs.len()),
            });
        }
        let flat: Vec<f32> = pairs.iter().flatten().copied().collect();
        let operands = self.owner.upload_owned(&flat)?;
        let results = self.owner.zeroed_owned::<f32>(pairs.len() * 3)?;
        self.owner.submit(
            &self.binary32,
            &[operands.buffer().reads(), results.buffer().writes()],
            Grid::linear(pairs.len() / 64, 64),
        )?;
        let mut values = vec![0.0_f32; pairs.len() * 3];
        self.owner.read_back(results.buffer(), &mut values)?;
        out.clear();
        out.extend(values.as_chunks::<3>().0.iter().copied());
        Ok(())
    }
}

/// Parameter bytes per device shard, at most; [`MetalBackend::with_shard_slots`]
/// may also bound its slots. A command's kernels run one after another, one
/// per shard a batch touches, and each adds a fixed latency (about 0.2 ms on
/// the preset), so a full shard is large: 6,718 F32 (13,437 F16) genomes of
/// the preset's 9,988-parameter topology. Shards start at
/// [`INITIAL_SHARD_SLOTS`] (clamped to the shard's maximum slots) and grow by
/// powers of two up to `SHARD_BYTES` as higher arena slots arrive.
pub const SHARD_BYTES: u64 = 256 << 20;

/// Initial slot capacity allocated when a device shard is first created
/// (clamped to `geometry.slots_per_shard()`). Shards grow by powers of two up
/// to `geometry.slots_per_shard()` (`SHARD_BYTES`) as higher arena slots
/// arrive, so rare mutated topologies consume ~1 MiB instead of eagerly
/// reserving the full 256 MiB slab.
pub const INITIAL_SHARD_SLOTS: usize = 64;

/// Under a budget, a shard holds at most this fraction of what a full
/// batch's staging leaves of it (FL-043): room for the five catalog
/// members' first shards and three more.
pub const SHARD_SHARE: u64 = 8;

/// The host bytes one shard holds beside its occupants: its job list and
/// its places in a batch's touched shards and kernel bindings.
const SHARD_HOST_BYTES: u64 = 256;

/// Target allocated slot capacity for a shard accommodating `max_local` within
/// a geometry holding at most `slots_per_shard` slots.
fn target_shard_slots(max_local: usize, slots_per_shard: usize) -> usize {
    let needed = max_local.saturating_add(1);
    let rounded = needed
        .checked_next_power_of_two()
        .unwrap_or(slots_per_shard)
        .max(INITIAL_SHARD_SLOTS);
    rounded.min(slots_per_shard)
}

/// Compute `(element_count, total_charged_bytes, device_bytes)` for a shard
/// holding `slot_capacity` slots of stride `stride` in `dtype`.
fn shard_accounting(
    slot_capacity: usize,
    stride: u64,
    dtype: WeightDtype,
) -> Result<(usize, u64, u64), CoreError> {
    let cap_u64 = u64::try_from(slot_capacity).map_err(|_| CoreError::Overflow {
        quantity: "arena shard slots",
    })?;
    let elements_u64 = cap_u64.checked_mul(stride).ok_or(CoreError::Overflow {
        quantity: "arena shard elements",
    })?;
    let elements = usize::try_from(elements_u64).map_err(|_| CoreError::Overflow {
        quantity: "arena shard elements",
    })?;
    let device_bytes = elements_u64.saturating_mul(dtype.bytes());
    let total_bytes = device_bytes
        .saturating_add(bytes_for::<Option<(u32, GenomeId)>>(slot_capacity))
        .saturating_add(SHARD_HOST_BYTES);
    Ok((elements, total_bytes, device_bytes))
}

/// Allocate a zeroed device parameter buffer for `elements` elements of `dtype`.
fn allocate_shard_parameters(
    owner: &DeviceOwner,
    dtype: WeightDtype,
    elements: usize,
) -> Result<ShardParameters, CoreError> {
    match dtype {
        WeightDtype::F32 => owner
            .zeroed_owned::<f32>(elements)
            .map(ShardParameters::F32),
        // F16 slot stride is a multiple of 4 elements, so `elements` is
        // always even and fills whole `u32` words.
        WeightDtype::F16 => owner
            .zeroed_owned::<u32>(elements / 2)
            .map(ShardParameters::F16),
    }
}

/// Highest `local` slot index in `&jobs[start_index..]` belonging to
/// `(topology_u32, shard_key)` whose shape, dtype, and parameter count match
/// the bucket and batch dimensions. Invoked only when a shard is created or
/// grown, so steady-state ticks pay zero scan overhead.
#[expect(
    clippy::too_many_arguments,
    reason = "filters lookahead by exact bucket geometry, dtype, and batch dimensions"
)]
fn batch_max_local_for_shard<J: JobView>(
    jobs: &[J],
    finite_flags: &[u8],
    start_index: usize,
    topology_u32: u32,
    shard_key: u64,
    slots_per_shard: u64,
    expected_dtype: WeightDtype,
    expected_len: u64,
    batch_dims: (usize, usize),
    initial_local: usize,
) -> usize {
    let Ok(max_possible) = usize::try_from(slots_per_shard) else {
        return initial_local;
    };
    if max_possible <= INITIAL_SHARD_SLOTS || initial_local.saturating_add(1) >= max_possible {
        return initial_local;
    }
    let Some(shard_base) = shard_key.checked_mul(slots_per_shard) else {
        return initial_local;
    };
    let shard_end = shard_base.checked_add(slots_per_shard);
    let mut max_local = initial_local;
    for (job, &is_finite) in jobs[start_index..].iter().zip(&finite_flags[start_index..]) {
        if is_finite == 0 {
            continue;
        }
        let s = job.slot();
        if s.topology() != topology_u32 {
            continue;
        }
        let raw = s.slot();
        if raw < shard_base || shard_end.is_some_and(|end| raw >= end) {
            continue;
        }
        let controller = job.controller();
        let topo = controller.topology();
        if (topo.input_width(), topo.raw_output_width()) != batch_dims
            || controller.parameters().dtype() != expected_dtype
            || controller.parameters().len() as u64 != expected_len
        {
            continue;
        }
        if let Ok(local) = usize::try_from(raw - shard_base)
            && local > max_local
        {
            max_local = local;
            if max_local.saturating_add(1) >= max_possible {
                break;
            }
        }
    }
    max_local
}

/// Current device staging buffer occupancy, used by
/// [`pending_device_staging_bytes`] to reserve headroom before creating or
/// growing a parameter shard.
#[derive(Clone, Copy)]
struct StagingState {
    input_capacity: usize,
    output_capacity: usize,
    fault_capacity: usize,
    has_io_buffers: bool,
    has_dims: bool,
}

/// Additional device bytes `MetalBackend::hold` and `dispatch_staged` will
/// allocate for a batch of at most `batch_jobs` jobs of `batch_dims` beyond
/// what the backend's staging buffers currently hold. Reserving this delta in
/// `choose_fitting_capacity` ensures a parameter shard never consumes the last
/// bytes of a tight `DeviceOwner` limit and causes `hold()` to fail later.
fn pending_device_staging_bytes(
    batch_jobs: usize,
    batch_dims: (usize, usize),
    state: StagingState,
    cur_shard_job_cap: usize,
) -> u64 {
    let (input_width, output_width) = batch_dims;
    let dims_delta = if state.has_dims {
        0
    } else {
        2 * size_of::<u32>() as u64
    };
    let need_inputs = batch_jobs.saturating_mul(input_width);
    let need_outputs = batch_jobs.saturating_mul(output_width);
    let io_delta = if state.has_io_buffers
        && batch_jobs <= state.fault_capacity
        && need_inputs <= state.input_capacity
        && need_outputs <= state.output_capacity
    {
        0
    } else {
        let organisms = batch_jobs
            .checked_next_power_of_two()
            .unwrap_or(batch_jobs)
            .max(256);
        let new_elems = organisms
            .saturating_mul(input_width)
            .max(need_inputs)
            .saturating_add(organisms.saturating_mul(output_width).max(need_outputs))
            .saturating_add(organisms);
        let new_bytes = (new_elems as u64).saturating_mul(size_of::<f32>() as u64);
        let old_elems = if state.has_io_buffers {
            state
                .input_capacity
                .saturating_add(state.output_capacity)
                .saturating_add(state.fault_capacity)
        } else {
            0
        };
        let old_bytes = (old_elems as u64).saturating_mul(size_of::<f32>() as u64);
        new_bytes.saturating_sub(old_bytes)
    };
    let shard_jobs_delta = if cur_shard_job_cap > 0 && batch_jobs <= cur_shard_job_cap {
        0
    } else {
        let cap = batch_jobs
            .checked_next_power_of_two()
            .unwrap_or(batch_jobs)
            .max(64);
        let new_bytes = (cap as u64)
            .saturating_mul(JOB_WORDS as u64)
            .saturating_mul(size_of::<u32>() as u64);
        let old_bytes = (cur_shard_job_cap as u64)
            .saturating_mul(JOB_WORDS as u64)
            .saturating_mul(size_of::<u32>() as u64);
        new_bytes.saturating_sub(old_bytes)
    };
    dims_delta
        .saturating_add(io_delta)
        .saturating_add(shard_jobs_delta)
}

/// Pick the largest power-of-two (or full-shard) slot capacity between
/// `target_shard_slots(local, slots_per_shard)` and
/// `target_shard_slots(batch_max_local, slots_per_shard)` that fits both
/// `budget_bytes` and `owner_limit`.
#[expect(
    clippy::too_many_arguments,
    reason = "self-contained shard capacity admission against both backend and device budgets"
)]
fn choose_fitting_capacity(
    local: usize,
    batch_max_local: usize,
    slots_per_shard: usize,
    stride: u64,
    dtype: WeightDtype,
    held_base: u64,
    staging: u64,
    budget_bytes: u64,
    owner_usage_base: u64,
    owner_limit: u64,
) -> Result<Option<(usize, usize, u64, u64)>, CoreError> {
    let min_cap = target_shard_slots(local, slots_per_shard);
    let mut cap = target_shard_slots(batch_max_local.max(local), slots_per_shard);
    loop {
        let (elements, total_bytes, device_bytes) = shard_accounting(cap, stride, dtype)?;
        let fits_budget = held_base
            .saturating_add(total_bytes)
            .saturating_add(staging)
            <= budget_bytes;
        let fits_owner = owner_usage_base.saturating_add(device_bytes) <= owner_limit;
        if fits_budget && fits_owner {
            return Ok(Some((cap, elements, total_bytes, device_bytes)));
        }
        if cap <= min_cap {
            return Ok(None);
        }
        let next = if cap.is_power_of_two() {
            cap / 2
        } else {
            1_usize << (usize::BITS - 1 - cap.leading_zeros())
        };
        cap = next.max(min_cap);
    }
}

/// Host snapshot of an existing shard's parameter buffer used to preserve all
/// `0..old_cap` resident weights bit-for-bit across power-of-two shard growth.
enum ShardSnapshot {
    F32(Vec<f32>),
    F16(Vec<u32>),
}

impl ShardSnapshot {
    fn read_from(
        owner: &DeviceOwner,
        parameters: &ShardParameters,
        old_elements: usize,
    ) -> Result<Self, CoreError> {
        match parameters {
            ShardParameters::F32(buf) => {
                let mut copy = Vec::new();
                copy.try_reserve_exact(old_elements)
                    .map_err(|_| CoreError::AllocationFailed {
                        buffer: "metal shard growth copy",
                        bytes: bytes_for::<f32>(old_elements),
                    })?;
                copy.resize(old_elements, 0.0);
                owner.read_back(buf.buffer(), &mut copy)?;
                Ok(Self::F32(copy))
            }
            ShardParameters::F16(buf) => {
                let old_words = old_elements / 2;
                let mut copy = Vec::new();
                copy.try_reserve_exact(old_words)
                    .map_err(|_| CoreError::AllocationFailed {
                        buffer: "metal shard growth copy",
                        bytes: bytes_for::<u32>(old_words),
                    })?;
                copy.resize(old_words, 0);
                owner.read_back(buf.buffer(), &mut copy)?;
                Ok(Self::F16(copy))
            }
        }
    }

    fn write_to(&self, owner: &DeviceOwner, parameters: &ShardParameters) -> Result<(), CoreError> {
        match (self, parameters) {
            (Self::F32(copy), ShardParameters::F32(buf)) => {
                owner.write_slice(buf.buffer(), 0, copy)
            }
            (Self::F16(copy), ShardParameters::F16(buf)) => {
                owner.write_slice(buf.buffer(), 0, copy)
            }
            _ => Err(CoreError::StateConflict {
                operation: "grow metal shard",
                detail: "shard parameter dtype changed during growth".to_owned(),
            }),
        }
    }
}

/// The most a batch's staging holds, device and host, for at most
/// `organisms` jobs over `shards` shards (FL-043): the per-organism device
/// buffers (bounded by [`MAX_SENSOR_WIDTH`] and [`MAX_CONTROLLER_OUTPUT_WIDTH`]),
/// a power of two of at least 256 rows; the 2-word `dims` uniform buffer;
/// each shard's job words, a power of two of at least 64 jobs; and the host's
/// staged inputs, outputs, fault flags, dispatch lists and job words, which
/// grow by doubling.
#[must_use]
pub const fn staging_bytes(organisms: u64, shards: u64) -> u64 {
    let rows = match organisms.checked_next_power_of_two() {
        Some(rows) if rows < 256 => 256,
        Some(rows) => rows,
        None => u64::MAX,
    };
    let row = ((MAX_SENSOR_WIDTH + MAX_CONTROLLER_OUTPUT_WIDTH + 1) * size_of::<f32>()) as u64;
    let words = (JOB_WORDS * size_of::<u32>()) as u64;
    let dims = (2 * size_of::<u32>()) as u64;
    let device = rows
        .saturating_mul(row)
        .saturating_add(dims)
        .saturating_add(
            organisms
                .saturating_mul(2)
                .saturating_add(shards.saturating_mul(64))
                .saturating_mul(words),
        );
    let host = organisms
        .saturating_mul(2)
        .saturating_mul(row + words + 2 * size_of::<usize>() as u64);
    device.saturating_add(host)
}

/// A shard's parameters on the device, in its bucket's weight dtype.
enum ShardParameters {
    F32(Owned<f32>),
    /// Binary16 values two to a word, the even element in the low half: the
    /// kernel reads the buffer as `half`. `ShardGeometry::fitting_for_dtype`
    /// pads every F16 slot stride to a multiple of 4 elements, so slots never
    /// share a word and always start at a `half4` boundary (Trap A).
    F16(Owned<u32>),
}

impl ShardParameters {
    /// The buffer, bound for a kernel to read.
    fn reads(&self) -> Binding {
        match self {
            Self::F32(values) => values.buffer().reads(),
            Self::F16(words) => words.buffer().reads(),
        }
    }
}

/// One shard of a topology's slots on the device: starts at
/// [`INITIAL_SHARD_SLOTS`] (clamped to `geometry.slots_per_shard()`), grows by
/// powers of two up to `geometry.slots_per_shard()` (`SHARD_BYTES`) as higher
/// slots arrive, and stays resident until every occupant retires
/// (`release_slots`, FL-043) or `forget` drops it.
struct DeviceShard {
    parameters: ShardParameters,
    /// Each allocated slot's uploaded occupant, its generation and genome, or
    /// `None` before its first upload or after retirement. Length equals
    /// `slot_capacity`.
    occupants: Vec<Option<(u32, GenomeId)>>,
    /// Currently allocated slot capacity (`<= geometry.slots_per_shard()`).
    slot_capacity: usize,
    /// Live uploaded occupants in `occupants`.
    live_count: usize,
    /// Exact bytes charged to `MetalBackend::held` for this shard's current
    /// `slot_capacity`.
    bytes: u64,
    /// This shard's job words for the batch, and their capacity in jobs.
    jobs: Option<Owned<u32>>,
    job_capacity: usize,
    staged: Vec<u32>,
}

/// Dense direct-indexed shard slots below this threshold (`0..256` covers
/// over 3.4M F16 genomes per topology with zero hashing); higher sparse shard
/// IDs (such as synthetic `slot >= 1 << 40` tests) fall back to `HashMap`.
const DENSE_SHARDS: usize = 256;

/// Per-topology shard map with O(1) direct indexing for normal arena shards
/// and a sparse `HashMap` fallback for arbitrary 64-bit shard IDs.
#[derive(Default)]
struct ShardTable {
    dense: Vec<Option<DeviceShard>>,
    sparse: HashMap<u64, DeviceShard>,
}

impl ShardTable {
    fn get(&self, key: u64) -> Option<&DeviceShard> {
        if let Ok(index) = usize::try_from(key)
            && index < DENSE_SHARDS
        {
            return self.dense.get(index).and_then(Option::as_ref);
        }
        self.sparse.get(&key)
    }

    fn get_mut(&mut self, key: u64) -> Option<&mut DeviceShard> {
        if let Ok(index) = usize::try_from(key)
            && index < DENSE_SHARDS
        {
            return self.dense.get_mut(index).and_then(Option::as_mut);
        }
        self.sparse.get_mut(&key)
    }

    fn insert(&mut self, key: u64, shard: DeviceShard) -> &mut DeviceShard {
        if let Ok(index) = usize::try_from(key)
            && index < DENSE_SHARDS
        {
            if self.dense.len() <= index {
                self.dense.resize_with(index + 1, || None);
            }
            return self.dense[index].insert(shard);
        }
        match self.sparse.entry(key) {
            Entry::Occupied(mut entry) => {
                entry.insert(shard);
                entry.into_mut()
            }
            Entry::Vacant(entry) => entry.insert(shard),
        }
    }

    fn remove(&mut self, key: u64) -> Option<DeviceShard> {
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

    fn values(&self) -> impl Iterator<Item = &DeviceShard> {
        self.dense.iter().flatten().chain(self.sparse.values())
    }

    fn values_mut(&mut self) -> impl Iterator<Item = &mut DeviceShard> {
        self.dense
            .iter_mut()
            .flatten()
            .chain(self.sparse.values_mut())
    }
}

/// One catalog member, as this backend first saw it.
struct DeviceBucket {
    topology: DenseTopology,
    /// The weight dtype of its first genome, which its shards hold; a job of
    /// another dtype is deferred.
    dtype: WeightDtype,
    /// Widest hidden layer of `topology`, selecting adaptive threadgroup size
    /// (`32` for `<= 64`, `64` for `> 64`).
    widest_hidden: usize,
    /// Validated `[hidden_layers, w0, w1]` words for the kernel's `jobs`
    /// buffer, cached once per topology; `None` for a shape the kernel refuses.
    shape_words: Option<[u32; 3]>,
    /// `None` when not one slot fits a device shard: its jobs are deferred.
    geometry: Option<ShardGeometry>,
    shards: ShardTable,
}

/// Minimum batch size before parallelizing host `f64 -> f32` input narrowing
/// and `f32 -> RawOutputs` output unpacking.
const PARALLEL_INPUT_MIN_JOBS: usize = 2_048;

/// Maximum concurrent chunks used for host input narrowing and output unpacking.
const PARALLEL_INPUT_MAX_THREADS: usize = 8;

/// Maximum touched shards dispatched from stack storage in [`MetalBackend::evaluate`]
/// without heap allocation for bindings and [`Kernel`] descriptors.
const INLINE_SHARDS: usize = 32;

/// Fixed per-topology intra-batch pointer cache slots (`0..8` covers the entire
/// 5-topology catalog with zero heap allocation and eliminates repeated
/// `DenseTopology` `Vec` equality comparisons on interleaved mutated worlds).
const TOPO_CACHE_SLOTS: usize = 8;

/// Uniform metadata view over [`ControllerJob`] and [`FlatControllerJob`] so
/// resident placement and per-shard staging share one zero-duplication core.
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

/// Cached host parallelism for `MetalBackend` staging when the caller did not
/// pass an explicit world worker budget via [`MetalBackend::set_staging_workers`].
static DEFAULT_STAGING_WORKERS: OnceLock<usize> = OnceLock::new();

fn default_staging_workers() -> usize {
    *DEFAULT_STAGING_WORKERS.get_or_init(|| {
        std::thread::available_parallelism()
            .map_or(4, usize::from)
            .clamp(1, PARALLEL_INPUT_MAX_THREADS)
    })
}

/// Active host write lease cached for one device parameter shard during
/// `place_and_stage` so multi-birth ticks amortize table-mutex / `Arc` lease
/// churn across consecutive uploads to the same shard.
enum ShardWriteLease {
    F32(WriteLease<f32>),
    F16(WriteLease<u32>),
}

/// [`MetalControllers`] behind the engine's controller port, with every
/// buffer resident (FL-026, plan §9.2). Each catalog member's genomes sit at
/// their arena slots ([`flife_neural::ArenaSlot`]) in that topology's device
/// shards, at most [`SHARD_BYTES`] to a shard and never more than a 32-bit word can
/// offset ([`flife_neural::ShardGeometry`]). A genome's parameters are
/// copied to the device once, the first time it is due, and kept until a
/// later occupant of its slot is copied over them; no parameter shard is
/// ever rebuilt from a batch. Each tick writes only the inputs and job words, and
/// runs one kernel per shard the batch touches, all in one command.
pub struct MetalBackend {
    controllers: MetalControllers,
    shard_slots: u64,
    budget: BackendBudget,
    /// Persistent `flife_core::pool` worker budget used for parallel input
    /// narrowing and output unpacking; matches the world's `cpu_workers` when
    /// constructed through [`ControllerChoice::backend`].
    staging_workers: usize,
    /// Shards made, and the bytes they hold, device and host.
    shard_count: u64,
    held: u64,
    buckets: Vec<Option<DeviceBucket>>,
    /// Per-organism buffers and their element capacities.
    input_capacity: usize,
    output_capacity: usize,
    fault_capacity: usize,
    inputs: Option<Owned<f32>>,
    raw: Option<Owned<f32>>,
    faults: Option<Owned<u32>>,
    /// Two-word `[input_width, raw_output_width]` uniform buffer.
    dims: Option<Owned<u32>>,
    /// Last `[input_width, raw_output_width]` written to `dims`, avoiding a
    /// redundant per-tick mutex/RwLock uniform upload when dimensions are steady.
    cached_dims: Option<[u32; 2]>,
    /// Host staging, reused.
    staged_inputs: Vec<f32>,
    input_finite: Vec<u8>,
    dispatched: Vec<usize>,
    touched: Vec<(usize, u64)>,
    packed_f16: Vec<u32>,
    values: Vec<f32>,
    flags: Vec<u32>,
    uploads: u64,
    /// The last batch's command: kernels run and device seconds.
    last_command: Option<(usize, f64)>,
}

impl std::fmt::Debug for MetalBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalBackend")
            .field("device", &self.controllers.device_name())
            .field("resident_genomes", &self.resident_genomes())
            .field("shards", &self.shards())
            .finish_non_exhaustive()
    }
}

impl MetalBackend {
    /// Open the default Metal device and compile the kernels.
    ///
    /// # Errors
    /// As [`MetalControllers::open`].
    pub fn open() -> Result<Self, CoreError> {
        Self::with_shard_slots(u64::MAX)
    }

    /// As [`MetalBackend::open`], with at most `shard_slots` slots to a
    /// shard (at least one), and still at most [`SHARD_BYTES`].
    ///
    /// # Errors
    /// As [`MetalControllers::open`].
    pub fn with_shard_slots(shard_slots: u64) -> Result<Self, CoreError> {
        Self::with_budget(shard_slots, BackendBudget::UNBOUNDED)
    }

    /// As [`MetalBackend::with_shard_slots`], holding at most `budget.bytes`
    /// device and host (FL-043, plan §19.4). Its owner refuses device bytes
    /// past it. A shard is at most a [`SHARD_SHARE`]th of what a batch of
    /// `budget.organisms` jobs leaves for parameters, and one that would
    /// leave too little for that batch's staging ([`staging_bytes`]) is not
    /// made: its jobs are deferred to the reference, which gives the same
    /// bits.
    ///
    /// The owner admits no more than the device's headroom as it opens
    /// ([`DeviceOwner::open`]), and a budget past it is cut to it, so the
    /// shards leave a batch's staging room on the device too
    /// ([`MetalBackend::budget`]). [`BackendBudget::UNBOUNDED`] is left
    /// whole, since its batches have no bound to stage for: its owner alone
    /// refuses past the headroom, and a shard it refuses is not made.
    ///
    /// # Errors
    /// As [`MetalControllers::open`].
    pub fn with_budget(shard_slots: u64, budget: BackendBudget) -> Result<Self, CoreError> {
        let owner = DeviceOwner::open(Limits {
            bytes: budget.bytes,
            leases_per_purpose: u32::MAX,
        })?;
        Self::with_owner(owner, shard_slots, budget)
    }

    /// As [`MetalBackend::with_budget`], sharing an existing [`DeviceOwner`]
    /// with the other Metal phase backends (`MetalField`, `MetalSensors`,
    /// `MetalUptake`, `MetalSettle`).
    ///
    /// # Errors
    /// As [`MetalControllers::with_owner`].
    pub fn with_owner(
        owner: DeviceOwner,
        shard_slots: u64,
        budget: BackendBudget,
    ) -> Result<Self, CoreError> {
        let controllers = MetalControllers::with_owner(owner)?;
        let budget = if budget == BackendBudget::UNBOUNDED {
            budget
        } else {
            BackendBudget {
                bytes: budget.bytes.min(controllers.owner().limits().bytes),
                ..budget
            }
        };
        Ok(Self {
            controllers,
            shard_slots: shard_slots.max(1),
            budget,
            staging_workers: default_staging_workers(),
            shard_count: 0,
            held: 0,
            buckets: Vec::new(),
            input_capacity: 0,
            output_capacity: 0,
            fault_capacity: 0,
            inputs: None,
            raw: None,
            faults: None,
            dims: None,
            cached_dims: None,
            staged_inputs: Vec::new(),
            input_finite: Vec::new(),
            dispatched: Vec::new(),
            touched: Vec::new(),
            packed_f16: Vec::new(),
            values: Vec::new(),
            flags: Vec::new(),
            uploads: 0,
            last_command: None,
        })
    }

    /// Pin the persistent `flife_core::pool` worker budget used for host input
    /// narrowing and output unpacking so this backend shares the world's single
    /// cached private worker pool (plan §3.4, §12.5).
    pub const fn set_staging_workers(&mut self, workers: NonZeroUsize) {
        self.staging_workers = workers.get();
    }

    /// Slots holding an uploaded genome. A dead genome's slot stays until a
    /// newborn takes it, so this follows each topology's peak live count.
    #[must_use]
    pub fn resident_genomes(&self) -> usize {
        self.buckets
            .iter()
            .flatten()
            .flat_map(|bucket| bucket.shards.values())
            .map(|shard| shard.occupants.iter().flatten().count())
            .sum()
    }

    /// Device shards across every topology.
    #[must_use]
    pub fn shards(&self) -> usize {
        self.buckets
            .iter()
            .flatten()
            .map(|bucket| bucket.shards.len())
            .sum()
    }

    /// Genomes copied to the device since the backend was opened or last
    /// forgot: once per genome while it lives, however often it is due.
    #[must_use]
    pub const fn uploads(&self) -> u64 {
        self.uploads
    }

    /// What it holds at most: the budget it was made with, cut to the
    /// device's headroom as it opened ([`MetalBackend::with_budget`]).
    #[must_use]
    pub const fn budget(&self) -> BackendBudget {
        self.budget
    }

    /// The owner of every buffer this backend holds.
    #[must_use]
    pub const fn owner(&self) -> &DeviceOwner {
        self.controllers.owner()
    }

    /// The last batch's command: the kernels it ran (one per shard it
    /// touched) and its command buffer's time on the device, in seconds.
    /// `None` before the first batch and after a batch that dispatched
    /// nothing.
    #[must_use]
    pub const fn last_command(&self) -> Option<(usize, f64)> {
        self.last_command
    }

    /// The bucket serving `job` inside `buckets`, made on its topology's first
    /// job; `None` for a job it defers.
    fn bucket_in<'a>(
        buckets: &'a mut Vec<Option<DeviceBucket>>,
        owner: &DeviceOwner,
        budget_share: u64,
        shard_slots: u64,
        job: &impl JobView,
    ) -> Result<Option<&'a mut DeviceBucket>, CoreError> {
        let Ok(index) = usize::try_from(job.slot().topology()) else {
            return Ok(None);
        };
        if buckets.len() <= index {
            let needed = index.checked_add(1).ok_or(CoreError::Overflow {
                quantity: "metal controller buckets",
            })?;
            let more = needed - buckets.len();
            reserve(buckets, more, "metal controller buckets")?;
            buckets.resize_with(needed, || None);
        }
        let controller = job.controller();
        let topology = controller.topology();
        let length = controller.parameters().len() as u64;
        let dtype = controller.parameters().dtype();
        let bucket = buckets[index].get_or_insert_with(|| {
            // A 32-bit job word offsets every element of a shard.
            let limit = (owner.max_buffer_bytes() as u64)
                .min(SHARD_BYTES)
                .min(budget_share)
                / dtype.bytes();
            let shape_words = usize::try_from(length)
                .ok()
                .and_then(|len| job_words(topology, len, 0, 0).ok())
                .map(|words| [words[2], words[3], words[4]]);
            let widest_hidden =
                topology.hidden_widths().iter().copied().max().unwrap_or(0) as usize;
            // Trap A defense-in-depth: `fitting_for_dtype` pads F16 slot stride
            // to a multiple of 4 elements, guaranteeing zero F16 alignment
            // deferrals even for odd-parameter topologies.
            DeviceBucket {
                topology: topology.clone(),
                dtype,
                widest_hidden,
                shape_words,
                geometry: shape_words.and_then(|_| {
                    ShardGeometry::fitting_for_dtype(
                        length,
                        dtype,
                        limit.min(u64::from(u32::MAX)),
                        shard_slots,
                    )
                    .ok()
                }),
                shards: ShardTable::default(),
            }
        });
        let serves = bucket.shape_words.is_some()
            && bucket.topology == *topology
            && bucket.dtype == dtype
            && bucket
                .geometry
                .is_some_and(|geometry| geometry.parameters() == length);
        Ok(serves.then_some(bucket))
    }

    /// Place every finite, shaped job matching `batch_dims` into its resident
    /// device shard and stage per-shard job words in a single pass when no live
    /// slot is overwritten mid-batch; falls back to the two-pass reconciliation
    /// when an existing occupant is overwritten (handling duplicate slots in
    /// one batch identically to the reference contract).
    ///
    /// When `external_finite` is `Some`, finiteness flags are read directly
    /// from that caller-supplied slice (zero copy on the flat-F32 path);
    /// otherwise `self.input_finite` (populated by [`narrow_inputs`]) is used.
    ///
    /// Writes touched `(topology, shard)` keys into `self.touched` (sorted) and
    /// returns `(all_dispatched, dispatched_count)`. Populates `self.dispatched`
    /// lazily only when a job is skipped or an existing slot is overwritten,
    /// avoiding per-tick index-vector writes on steady-state full-dispatch ticks.
    #[expect(
        clippy::too_many_lines,
        reason = "single-pass placement and staging with duplicate-slot fallback and cached shard lease"
    )]
    fn place_and_stage<J: JobView + Sync>(
        &mut self,
        jobs: &[J],
        external_finite: Option<&[u8]>,
        batch_dims: (usize, usize),
    ) -> Result<(bool, usize), CoreError> {
        if u32::try_from(jobs.len()).is_err() {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal controller batch",
                detail: format!("batch size {} exceeds u32", jobs.len()),
            });
        }
        if self.staging_workers > 1
            && jobs.len() >= 4_096
            && self.shard_count > 0
            && self.buckets.len() <= TOPO_CACHE_SLOTS
            && let Some(done) = self.place_and_stage_parallel(jobs, external_finite, batch_dims)?
        {
            return Ok(done);
        }
        let Self {
            controllers,
            shard_slots,
            budget,
            shard_count,
            held,
            buckets,
            input_capacity,
            output_capacity,
            fault_capacity,
            inputs,
            raw,
            faults,
            dims,
            input_finite,
            dispatched,
            touched,
            packed_f16: packed,
            uploads,
            ..
        } = self;
        let staging_state = StagingState {
            input_capacity: *input_capacity,
            output_capacity: *output_capacity,
            fault_capacity: *fault_capacity,
            has_io_buffers: inputs.is_some() && raw.is_some() && faults.is_some(),
            has_dims: dims.is_some(),
        };
        let finite_flags = external_finite.unwrap_or(input_finite.as_slice());
        if finite_flags.len() < jobs.len() {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal controller input flags",
                detail: format!(
                    "expected at least {} finite flags, got {}",
                    jobs.len(),
                    finite_flags.len()
                ),
            });
        }
        let owner = &controllers.owner;
        let budget = *budget;
        let shard_slots = *shard_slots;
        for bucket in buckets.iter_mut().flatten() {
            for shard in bucket.shards.values_mut() {
                shard.staged.clear();
            }
        }
        dispatched.clear();
        touched.clear();
        let mut dispatched_count = 0_usize;
        let mut tracking_dispatched = false;
        let mut overwrote_existing = false;
        let mut all_dispatched = true;
        let budget_share = match budget.bytes {
            u64::MAX => u64::MAX,
            bytes => bytes.saturating_sub(staging_bytes(budget.organisms, 0)) / SHARD_SHARE,
        };
        // Safe intra-batch per-topology cache: every `&'a DenseTopology`
        // borrowed inside `jobs` stays alive and immutable for the whole call,
        // so an equal pointer with matching bucket index, dtype, and parameter
        // count skips repeating `DenseTopology`'s `Vec` equality comparisons
        // even when mutated topologies interleave across organisms.
        let mut cached_topos: [Option<(*const DenseTopology, WeightDtype, u64)>; TOPO_CACHE_SLOTS] =
            [None; TOPO_CACHE_SLOTS];
        let mut cached_topo_overflow: Option<(usize, *const DenseTopology, WeightDtype, u64)> =
            None;
        // Per-topology division-free shard range cache `(shard, base_slot, end_slot)`
        // eliminating per-organism 64-bit integer divisions on multi-shard worlds.
        let mut cached_shard_ranges: [Option<(u64, u64, u64)>; TOPO_CACHE_SLOTS] =
            [None; TOPO_CACHE_SLOTS];
        let mut cached_shard_overflow: Option<(usize, u64, u64, u64)> = None;
        // Cached write lease for the shard currently receiving newborn uploads;
        // at most one lease is held at a time and it is dropped before any
        // new shard allocation or before `place_and_stage` returns.
        let mut active_lease: Option<(usize, u64, ShardWriteLease)> = None;

        for (index, (job, &is_finite)) in jobs.iter().zip(finite_flags).enumerate() {
            if is_finite == 0 {
                all_dispatched = false;
                continue;
            }
            let controller = job.controller();
            let job_topo = controller.topology();
            if (job_topo.input_width(), job_topo.raw_output_width()) != batch_dims {
                all_dispatched = false;
                continue;
            }
            let slot = job.slot();
            let Ok(topology) = usize::try_from(slot.topology()) else {
                all_dispatched = false;
                continue;
            };
            let job_dtype = controller.parameters().dtype();
            let job_len = controller.parameters().len() as u64;
            let job_ptr = std::ptr::from_ref(job_topo);
            let hit_cache = if topology < TOPO_CACHE_SLOTS {
                matches!(
                    cached_topos[topology],
                    Some((ptr, dt, len))
                        if std::ptr::eq(ptr, job_ptr) && dt == job_dtype && len == job_len
                )
            } else {
                matches!(
                    cached_topo_overflow,
                    Some((idx, ptr, dt, len))
                        if idx == topology
                            && std::ptr::eq(ptr, job_ptr)
                            && dt == job_dtype
                            && len == job_len
                )
            };
            let Some(bucket) = (if hit_cache {
                buckets.get_mut(topology).and_then(Option::as_mut)
            } else {
                let found = Self::bucket_in(buckets, owner, budget_share, shard_slots, job)?;
                if found.is_some() {
                    if topology < TOPO_CACHE_SLOTS {
                        cached_topos[topology] = Some((job_ptr, job_dtype, job_len));
                    } else {
                        cached_topo_overflow = Some((topology, job_ptr, job_dtype, job_len));
                    }
                }
                found
            }) else {
                all_dispatched = false;
                continue;
            };
            let (Some(geometry), Some(shape_words)) = (bucket.geometry, bucket.shape_words) else {
                all_dispatched = false;
                continue;
            };
            let dtype = bucket.dtype;
            let slots_per_shard = geometry.slots_per_shard();
            let stride = geometry.slot_stride_elements();
            let raw_slot = slot.slot();
            let (shard_key, local_u64, offset_u64) = if raw_slot < slots_per_shard {
                (0_u64, raw_slot, raw_slot * stride)
            } else if topology < TOPO_CACHE_SLOTS
                && let Some((cs, base, end)) = cached_shard_ranges[topology]
                && raw_slot >= base
                && raw_slot < end
            {
                let local = raw_slot - base;
                (cs, local, local * stride)
            } else if topology >= TOPO_CACHE_SLOTS
                && let Some((ct, cs, base, end)) = cached_shard_overflow
                && ct == topology
                && raw_slot >= base
                && raw_slot < end
            {
                let local = raw_slot - base;
                (cs, local, local * stride)
            } else {
                let at = geometry.locate(raw_slot);
                if let Some(base) = at.shard.checked_mul(slots_per_shard)
                    && let Some(end) = base.checked_add(slots_per_shard)
                {
                    if topology < TOPO_CACHE_SLOTS {
                        cached_shard_ranges[topology] = Some((at.shard, base, end));
                    } else {
                        cached_shard_overflow = Some((topology, at.shard, base, end));
                    }
                }
                (at.shard, at.slot_in_shard, at.offset)
            };
            let local = usize::try_from(local_u64).map_err(|_| CoreError::Overflow {
                quantity: "arena slot",
            })?;
            let slots_per_shard_usize =
                usize::try_from(slots_per_shard).map_err(|_| CoreError::Overflow {
                    quantity: "arena shard slots",
                })?;
            let needs_create_or_grow = match bucket.shards.get(shard_key) {
                None => Some(None),
                Some(shard) if local >= shard.slot_capacity => Some(Some(shard.slot_capacity)),
                Some(_) => None,
            };
            if let Some(existing_cap) = needs_create_or_grow {
                active_lease = None;
                if local >= slots_per_shard_usize {
                    all_dispatched = false;
                    continue;
                }
                let batch_max_local = batch_max_local_for_shard(
                    jobs,
                    finite_flags,
                    index,
                    slot.topology(),
                    shard_key,
                    slots_per_shard,
                    dtype,
                    geometry.parameters(),
                    batch_dims,
                    local,
                );
                let owner_limit = owner.limits().bytes;
                if let Some(old_cap) = existing_cap {
                    // Grow an existing shard by powers of two, preserving all
                    // `0..old_cap` resident weights bit-for-bit.
                    let (old_elements, old_bytes, old_device_bytes, cur_shard_job_cap) =
                        match bucket.shards.get(shard_key) {
                            Some(shard) => {
                                let (old_elems, _, old_dev) =
                                    shard_accounting(old_cap, stride, dtype)?;
                                (
                                    old_elems,
                                    shard.bytes,
                                    old_dev,
                                    if shard.jobs.is_some() {
                                        shard.job_capacity
                                    } else {
                                        0
                                    },
                                )
                            }
                            None => {
                                all_dispatched = false;
                                continue;
                            }
                        };
                    let pending_staging = pending_device_staging_bytes(
                        jobs.len(),
                        batch_dims,
                        staging_state,
                        cur_shard_job_cap,
                    );
                    let staging = staging_bytes(budget.organisms, *shard_count);
                    let Some((new_cap, new_elements, new_bytes, new_device_bytes)) =
                        choose_fitting_capacity(
                            local,
                            batch_max_local,
                            slots_per_shard_usize,
                            stride,
                            dtype,
                            held.saturating_sub(old_bytes),
                            staging,
                            budget.bytes,
                            owner
                                .usage()
                                .bytes
                                .saturating_sub(old_device_bytes)
                                .saturating_add(pending_staging),
                            owner_limit,
                        )?
                    else {
                        all_dispatched = false;
                        continue;
                    };
                    let mut new_occupants = Vec::new();
                    new_occupants.try_reserve_exact(new_cap).map_err(|_| {
                        CoreError::AllocationFailed {
                            buffer: "metal controller slots",
                            bytes: bytes_for::<Option<(u32, GenomeId)>>(new_cap),
                        }
                    })?;
                    let snapshot = {
                        let Some(shard) = bucket.shards.get(shard_key) else {
                            all_dispatched = false;
                            continue;
                        };
                        new_occupants.extend_from_slice(&shard.occupants);
                        new_occupants.resize(new_cap, None);
                        ShardSnapshot::read_from(owner, &shard.parameters, old_elements)?
                    };
                    if owner.usage().bytes.saturating_add(new_device_bytes) <= owner_limit {
                        let new_parameters =
                            match allocate_shard_parameters(owner, dtype, new_elements) {
                                Ok(parameters) => parameters,
                                Err(CoreError::BudgetExhausted { .. }) => {
                                    all_dispatched = false;
                                    continue;
                                }
                                Err(error) => return Err(error),
                            };
                        snapshot.write_to(owner, &new_parameters)?;
                        if let Some(shard) = bucket.shards.get_mut(shard_key) {
                            shard.parameters = new_parameters;
                            shard.occupants = new_occupants;
                            shard.slot_capacity = new_cap;
                            shard.bytes = new_bytes;
                            *held = held.saturating_sub(old_bytes).saturating_add(new_bytes);
                        }
                    } else if let Some(removed) = bucket.shards.remove(shard_key) {
                        // Tight device limit: release the old parameter buffer
                        // before allocating the larger replacement while
                        // preserving `staged`, `jobs`, `live_count`, and
                        // `occupants[0..old_cap]`.
                        let DeviceShard {
                            parameters: old_parameters,
                            occupants: _old_occupants,
                            slot_capacity: _,
                            live_count,
                            bytes: removed_bytes,
                            jobs: shard_jobs,
                            job_capacity,
                            staged,
                        } = removed;
                        *held = held.saturating_sub(removed_bytes);
                        *shard_count = shard_count.saturating_sub(1);
                        drop(old_parameters);
                        let new_parameters =
                            match allocate_shard_parameters(owner, dtype, new_elements) {
                                Ok(parameters) => parameters,
                                Err(CoreError::BudgetExhausted { .. }) => {
                                    if live_count > 0 || !staged.is_empty() {
                                        overwrote_existing = true;
                                    }
                                    all_dispatched = false;
                                    continue;
                                }
                                Err(error) => return Err(error),
                            };
                        snapshot.write_to(owner, &new_parameters)?;
                        *held = held.saturating_add(new_bytes);
                        *shard_count = shard_count.saturating_add(1);
                        bucket.shards.insert(
                            shard_key,
                            DeviceShard {
                                parameters: new_parameters,
                                occupants: new_occupants,
                                slot_capacity: new_cap,
                                live_count,
                                bytes: new_bytes,
                                jobs: shard_jobs,
                                job_capacity,
                                staged,
                            },
                        );
                    }
                } else {
                    let pending_staging =
                        pending_device_staging_bytes(jobs.len(), batch_dims, staging_state, 0);
                    let staging = staging_bytes(budget.organisms, shard_count.saturating_add(1));
                    let Some((cap, elements, bytes, _device_bytes)) = choose_fitting_capacity(
                        local,
                        batch_max_local,
                        slots_per_shard_usize,
                        stride,
                        dtype,
                        *held,
                        staging,
                        budget.bytes,
                        owner.usage().bytes.saturating_add(pending_staging),
                        owner_limit,
                    )?
                    else {
                        all_dispatched = false;
                        continue;
                    };
                    let parameters = match allocate_shard_parameters(owner, dtype, elements) {
                        Ok(parameters) => parameters,
                        Err(CoreError::BudgetExhausted { .. }) => {
                            all_dispatched = false;
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    let mut occupants = Vec::new();
                    occupants
                        .try_reserve_exact(cap)
                        .map_err(|_| CoreError::AllocationFailed {
                            buffer: "metal controller slots",
                            bytes: bytes_for::<Option<(u32, GenomeId)>>(cap),
                        })?;
                    occupants.resize(cap, None);
                    // Commit VRAM accounting atomically with shard insertion so
                    // an early `?` on a later write/reserve can never desync
                    // `held` or `shard_count` from `buckets`.
                    *held = held.saturating_add(bytes);
                    *shard_count = shard_count.saturating_add(1);
                    bucket.shards.insert(
                        shard_key,
                        DeviceShard {
                            parameters,
                            occupants,
                            slot_capacity: cap,
                            live_count: 0,
                            bytes,
                            jobs: None,
                            job_capacity: 0,
                            staged: Vec::new(),
                        },
                    );
                }
            }
            let Some(shard) = bucket.shards.get_mut(shard_key) else {
                all_dispatched = false;
                continue;
            };
            let own = Some((slot.generation(), job.genome()));
            let Some(occupant) = shard.occupants.get_mut(local) else {
                all_dispatched = false;
                continue;
            };
            if *occupant != own {
                let offset = usize::try_from(offset_u64).map_err(|_| CoreError::Overflow {
                    quantity: "arena offset",
                })?;
                let dtype_matches = matches!(
                    (&shard.parameters, controller.parameters()),
                    (ShardParameters::F32(_), Weights::F32(_))
                        | (ShardParameters::F16(_), Weights::F16(_))
                );
                if !dtype_matches {
                    if occupant.take().is_some() {
                        shard.live_count = shard.live_count.saturating_sub(1);
                        overwrote_existing = true;
                    }
                    all_dispatched = false;
                    continue;
                }
                // The old occupant is gone before the new one is written, so a
                // failure leaves the slot empty, never mixed.
                if occupant.take().is_some() {
                    shard.live_count = shard.live_count.saturating_sub(1);
                    overwrote_existing = true;
                }
                let reuse_lease = matches!(
                    active_lease,
                    Some((lt, ls, _)) if lt == topology && ls == shard_key
                );
                if !reuse_lease {
                    drop(active_lease.take());
                    let lease = match &shard.parameters {
                        ShardParameters::F32(buffer) => {
                            ShardWriteLease::F32(owner.write(buffer.buffer())?)
                        }
                        ShardParameters::F16(words) => {
                            ShardWriteLease::F16(owner.write(words.buffer())?)
                        }
                    };
                    active_lease = Some((topology, shard_key, lease));
                }
                match (
                    active_lease.as_mut().map(|(_, _, lease)| lease),
                    controller.parameters(),
                ) {
                    (Some(ShardWriteLease::F32(lease)), Weights::F32(values)) => {
                        lease.write(offset, values)?;
                    }
                    // Two binary16 values a word, the even one low; F16 slot
                    // stride is a multiple of 4 elements, so `offset` is always
                    // even (and 4-aligned). An odd `values.len()` writes its
                    // trailing element into the low 16 bits of the last word
                    // with zero high-half padding inside this slot's stride.
                    (Some(ShardWriteLease::F16(lease)), Weights::F16(values)) => {
                        packed.clear();
                        let word_count = values.len().div_ceil(2);
                        if packed.capacity() < word_count {
                            reserve(packed, word_count, "metal binary16 words")?;
                        }
                        let (pairs, remainder) = values.as_chunks::<2>();
                        packed.extend(pairs.iter().map(|[low, high]| {
                            u32::from(low.to_bits()) | (u32::from(high.to_bits()) << 16)
                        }));
                        if let Some(&last) = remainder.first() {
                            packed.push(u32::from(last.to_bits()));
                        }
                        lease.write(offset / 2, packed)?;
                    }
                    _ => {
                        all_dispatched = false;
                        continue;
                    }
                }
                *occupant = own;
                shard.live_count += 1;
                *uploads += 1;
            }
            let row = dispatched_count;
            dispatched_count += 1;
            if !all_dispatched || overwrote_existing {
                if !tracking_dispatched {
                    if dispatched.capacity() < jobs.len() {
                        reserve(dispatched, jobs.len(), "metal controller dispatch")?;
                    }
                    dispatched.extend(0..row);
                    tracking_dispatched = true;
                }
                dispatched.push(index);
            }
            if !overwrote_existing {
                let row_word = u32::try_from(row).map_err(|_| CoreError::Overflow {
                    quantity: "metal controller batch row",
                })?;
                let offset_word = u32::try_from(offset_u64).map_err(|_| CoreError::Overflow {
                    quantity: "metal controller parameter offset",
                })?;
                if shard.staged.is_empty() {
                    if touched.capacity() == touched.len() {
                        reserve(touched, 1, "metal controller shards")?;
                    }
                    touched.push((topology, shard_key));
                }
                if shard.staged.capacity() - shard.staged.len() < JOB_WORDS {
                    reserve(&mut shard.staged, JOB_WORDS, "metal jobs")?;
                }
                shard.staged.extend_from_slice(&[
                    row_word,
                    offset_word,
                    shape_words[0],
                    shape_words[1],
                    shape_words[2],
                ]);
            }
        }
        drop(active_lease);

        if (!all_dispatched || overwrote_existing) && !tracking_dispatched && dispatched_count > 0 {
            if dispatched.capacity() < dispatched_count {
                reserve(dispatched, dispatched_count, "metal controller dispatch")?;
            }
            dispatched.extend(0..dispatched_count);
        }

        if overwrote_existing {
            // At least one live slot was overwritten during this batch; drop
            // any earlier job whose slot was evicted by a later job in the
            // same batch and rebuild per-shard job lists cleanly.
            touched.clear();
            for bucket in buckets.iter_mut().flatten() {
                for shard in bucket.shards.values_mut() {
                    shard.staged.clear();
                }
            }
            let candidates = std::mem::take(dispatched);
            for &index in &candidates {
                let Some(job) = jobs.get(index) else { continue };
                let slot = job.slot();
                let Ok(topology) = usize::try_from(slot.topology()) else {
                    all_dispatched = false;
                    continue;
                };
                let Some(bucket) = buckets.get_mut(topology).and_then(Option::as_mut) else {
                    all_dispatched = false;
                    continue;
                };
                let (Some(geometry), Some(shape_words)) = (bucket.geometry, bucket.shape_words)
                else {
                    all_dispatched = false;
                    continue;
                };
                let at = geometry.locate(slot.slot());
                let Some(shard) = bucket.shards.get_mut(at.shard) else {
                    all_dispatched = false;
                    continue;
                };
                let resident = usize::try_from(at.slot_in_shard)
                    .ok()
                    .and_then(|local| shard.occupants.get(local).copied())
                    .flatten()
                    == Some((slot.generation(), job.genome()));
                if !resident {
                    all_dispatched = false;
                    continue;
                }
                let row = dispatched.len();
                dispatched.push(index);
                let row_word = u32::try_from(row).map_err(|_| CoreError::Overflow {
                    quantity: "metal controller batch row",
                })?;
                let offset_word = u32::try_from(at.offset).map_err(|_| CoreError::Overflow {
                    quantity: "metal controller parameter offset",
                })?;
                if shard.staged.is_empty() {
                    if touched.capacity() == touched.len() {
                        reserve(touched, 1, "metal controller shards")?;
                    }
                    touched.push((topology, at.shard));
                }
                if shard.staged.capacity() - shard.staged.len() < JOB_WORDS {
                    reserve(&mut shard.staged, JOB_WORDS, "metal jobs")?;
                }
                shard.staged.extend_from_slice(&[
                    row_word,
                    offset_word,
                    shape_words[0],
                    shape_words[1],
                    shape_words[2],
                ]);
            }
            dispatched_count = dispatched.len();
        }
        if touched.len() > 1 && !touched.is_sorted() {
            touched.sort_unstable();
        }
        Ok((all_dispatched, dispatched_count))
    }

    /// Steady-state parallel fast path for [`Self::place_and_stage`]: splits
    /// read-only occupant verification (Pass A) and per-shard `JOB_WORDS`
    /// emission (Pass B) across `self.staging_workers` on `flife_core::pool`,
    /// handling small batches of empty-slot newborns serially in between.
    ///
    /// Produces word-for-word identical `shard.staged` and `touched` lists to
    /// the serial loop with zero persistent memory overhead, and returns
    /// `Ok(None)` without mutating state whenever any non-steady edge case
    /// (nonfinite input, unallocated/undersized shard, eviction overwrite,
    /// mismatched dtype/dims, or bulk initial upload) requires the serial path.
    #[expect(
        clippy::too_many_lines,
        reason = "two-pass parallel resident placement and shard staging with zero-mutation fallback"
    )]
    fn place_and_stage_parallel<J: JobView + Sync>(
        &mut self,
        jobs: &[J],
        external_finite: Option<&[u8]>,
        batch_dims: (usize, usize),
    ) -> Result<Option<(bool, usize)>, CoreError> {
        const MAX_FAST_SHARDS: usize = 32;
        const MAX_FAST_DENSE_SHARDS: usize = 32;
        const MAX_FAST_NEWBORNS: usize = 64;

        #[derive(Clone, Copy)]
        struct FastTopoInfo {
            geometry: ShardGeometry,
            dtype: WeightDtype,
            shape_words: [u32; 3],
            params: u64,
            slots_per_shard: u64,
            stride: u64,
        }

        #[derive(Clone, Copy)]
        struct NewbornJob {
            job_idx: usize,
            active_shard: usize,
            local: usize,
            offset: u64,
        }

        let finite_flags = external_finite.unwrap_or(self.input_finite.as_slice());
        if finite_flags.len() < jobs.len() {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal controller input flags",
                detail: format!(
                    "expected at least {} finite flags, got {}",
                    jobs.len(),
                    finite_flags.len()
                ),
            });
        }

        let mut active_shards = [(0_usize, 0_usize); MAX_FAST_SHARDS];
        let mut active_len = 0_usize;
        let mut shard_map = [[u8::MAX; MAX_FAST_DENSE_SHARDS]; TOPO_CACHE_SLOTS];
        let mut topo_info: [Option<FastTopoInfo>; TOPO_CACHE_SLOTS] = [None; TOPO_CACHE_SLOTS];

        for (topology, bucket_opt) in self.buckets.iter().enumerate() {
            let Some(bucket) = bucket_opt.as_ref() else {
                continue;
            };
            if !bucket.shards.sparse.is_empty() {
                return Ok(None);
            }
            let (Some(geometry), Some(shape_words)) = (bucket.geometry, bucket.shape_words) else {
                if bucket.shards.dense.iter().any(Option::is_some) {
                    return Ok(None);
                }
                continue;
            };
            topo_info[topology] = Some(FastTopoInfo {
                geometry,
                dtype: bucket.dtype,
                shape_words,
                params: geometry.parameters(),
                slots_per_shard: geometry.slots_per_shard(),
                stride: geometry.slot_stride_elements(),
            });
            for (shard_idx, shard_opt) in bucket.shards.dense.iter().enumerate() {
                if shard_opt.is_none() {
                    continue;
                }
                if shard_idx >= MAX_FAST_DENSE_SHARDS || active_len >= MAX_FAST_SHARDS {
                    return Ok(None);
                }
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "active_len < MAX_FAST_SHARDS (32) <= u8::MAX"
                )]
                {
                    shard_map[topology][shard_idx] = active_len as u8;
                }
                active_shards[active_len] = (topology, shard_idx);
                active_len += 1;
            }
        }
        if active_len == 0 {
            return Ok(None);
        }

        let workers = self.staging_workers.max(1);
        let chunk_count = workers
            .saturating_mul(2)
            .min(32)
            .min(jobs.len().div_ceil(512))
            .max(1);
        let chunk_len = jobs.len().div_ceil(chunk_count);

        // Pass A (read-only on `&self.buckets`): verify inputs, topology, and
        // shard residency, count staged jobs per active shard, and collect any
        // empty-slot newborns.
        let (chunk_counts, newborns): (Vec<[usize; MAX_FAST_SHARDS]>, Vec<NewbornJob>) = {
            let mut topo_refs: [Option<&DenseTopology>; TOPO_CACHE_SLOTS] =
                [None; TOPO_CACHE_SLOTS];
            for (topology, bucket_opt) in self.buckets.iter().enumerate() {
                if let Some(bucket) = bucket_opt.as_ref() {
                    topo_refs[topology] = Some(&bucket.topology);
                }
            }
            let mut occupant_slices: [&[Option<(u32, GenomeId)>]; MAX_FAST_SHARDS] =
                [&[]; MAX_FAST_SHARDS];
            for s in 0..active_len {
                let (topology, shard_idx) = active_shards[s];
                if let Some(shard) = self.buckets[topology]
                    .as_ref()
                    .and_then(|b| b.shards.dense.get(shard_idx))
                    .and_then(Option::as_ref)
                {
                    occupant_slices[s] = shard.occupants.as_slice();
                } else {
                    return Ok(None);
                }
            }

            struct VerifyChunk<'a, J> {
                base: usize,
                jobs: &'a [J],
                finite: &'a [u8],
                counts: [usize; MAX_FAST_SHARDS],
                newborns: Vec<NewbornJob>,
                fallback: bool,
            }
            let mut tasks: Vec<VerifyChunk<'_, J>> = jobs
                .chunks(chunk_len)
                .zip(finite_flags.chunks(chunk_len))
                .enumerate()
                .map(|(idx, (j_chunk, f_chunk))| VerifyChunk {
                    base: idx * chunk_len,
                    jobs: j_chunk,
                    finite: f_chunk,
                    counts: [0; MAX_FAST_SHARDS],
                    newborns: Vec::new(),
                    fallback: false,
                })
                .collect();

            pool::parallel_tasks(workers, "metal place verify", &mut tasks, |_, task| {
                let mut cached_topos: [Option<(*const DenseTopology, WeightDtype, u64)>;
                    TOPO_CACHE_SLOTS] = [None; TOPO_CACHE_SLOTS];
                for (local_i, (job, &is_finite)) in
                    task.jobs.iter().zip(task.finite.iter()).enumerate()
                {
                    if is_finite == 0 {
                        task.fallback = true;
                        return;
                    }
                    let slot = job.slot();
                    let topology = slot.topology() as usize;
                    if topology >= TOPO_CACHE_SLOTS {
                        task.fallback = true;
                        return;
                    }
                    let Some(info) = topo_info[topology] else {
                        task.fallback = true;
                        return;
                    };
                    let controller = job.controller();
                    let job_topo = controller.topology();
                    if (job_topo.input_width(), job_topo.raw_output_width()) != batch_dims {
                        task.fallback = true;
                        return;
                    }
                    let job_dtype = controller.parameters().dtype();
                    let job_len = controller.parameters().len() as u64;
                    let job_ptr = std::ptr::from_ref(job_topo);
                    let hit = matches!(
                        cached_topos[topology],
                        Some((ptr, dt, len))
                            if std::ptr::eq(ptr, job_ptr) && dt == job_dtype && len == job_len
                    );
                    if !hit {
                        let Some(bucket_topo) = topo_refs[topology] else {
                            task.fallback = true;
                            return;
                        };
                        if job_dtype != info.dtype
                            || job_len != info.params
                            || job_topo != bucket_topo
                        {
                            task.fallback = true;
                            return;
                        }
                        cached_topos[topology] = Some((job_ptr, job_dtype, job_len));
                    }
                    let raw_slot = slot.slot();
                    let (shard_idx_u64, local_u64, offset_u64) = if raw_slot < info.slots_per_shard
                    {
                        (0_u64, raw_slot, raw_slot * info.stride)
                    } else {
                        let at = info.geometry.locate(raw_slot);
                        (at.shard, at.slot_in_shard, at.offset)
                    };
                    if shard_idx_u64 >= MAX_FAST_DENSE_SHARDS as u64
                        || u32::try_from(offset_u64).is_err()
                    {
                        task.fallback = true;
                        return;
                    }
                    let active_id = shard_map[topology][shard_idx_u64 as usize];
                    if active_id == u8::MAX {
                        task.fallback = true;
                        return;
                    }
                    let s = active_id as usize;
                    let Ok(local) = usize::try_from(local_u64) else {
                        task.fallback = true;
                        return;
                    };
                    let Some(&occupant) = occupant_slices[s].get(local) else {
                        task.fallback = true;
                        return;
                    };
                    let own = Some((slot.generation(), job.genome()));
                    if occupant == own {
                        task.counts[s] += 1;
                    } else if occupant.is_none() {
                        if task.newborns.len() >= MAX_FAST_NEWBORNS {
                            task.fallback = true;
                            return;
                        }
                        task.newborns.push(NewbornJob {
                            job_idx: task.base + local_i,
                            active_shard: s,
                            local,
                            offset: offset_u64,
                        });
                        task.counts[s] += 1;
                    } else {
                        task.fallback = true;
                        return;
                    }
                }
            })?;

            if tasks.iter().any(|task| task.fallback) {
                return Ok(None);
            }
            let total_newborns: usize = tasks.iter().map(|t| t.newborns.len()).sum();
            if total_newborns > MAX_FAST_NEWBORNS {
                return Ok(None);
            }
            let mut all_counts = Vec::with_capacity(tasks.len());
            let mut all_newborns = Vec::with_capacity(total_newborns);
            for mut task in tasks {
                all_counts.push(task.counts);
                all_newborns.append(&mut task.newborns);
            }
            (all_counts, all_newborns)
        };

        // Pre-validate every newborn before mutating any shard state.
        if !newborns.is_empty() {
            for (i, nb) in newborns.iter().enumerate() {
                if newborns[..i]
                    .iter()
                    .any(|prev| prev.active_shard == nb.active_shard && prev.local == nb.local)
                {
                    return Ok(None);
                }
                if usize::try_from(nb.offset).is_err() {
                    return Ok(None);
                }
                let (topology, shard_idx) = active_shards[nb.active_shard];
                let Some(shard) = self.buckets[topology]
                    .as_ref()
                    .and_then(|b| b.shards.dense.get(shard_idx))
                    .and_then(Option::as_ref)
                else {
                    return Ok(None);
                };
                if shard.occupants.get(nb.local).copied().flatten().is_some() {
                    return Ok(None);
                }
                let dtype_matches = matches!(
                    (
                        &shard.parameters,
                        jobs[nb.job_idx].controller().parameters()
                    ),
                    (ShardParameters::F32(_), Weights::F32(_))
                        | (ShardParameters::F16(_), Weights::F16(_))
                );
                if !dtype_matches {
                    return Ok(None);
                }
            }
            let owner = &self.controllers.owner;
            let mut active_lease: Option<(usize, ShardWriteLease)> = None;
            for nb in &newborns {
                let (topology, shard_idx) = active_shards[nb.active_shard];
                let Some(shard) = self.buckets[topology]
                    .as_mut()
                    .and_then(|b| b.shards.dense.get_mut(shard_idx))
                    .and_then(Option::as_mut)
                else {
                    return Ok(None);
                };
                let offset = usize::try_from(nb.offset).map_err(|_| CoreError::Overflow {
                    quantity: "arena offset",
                })?;
                if !matches!(active_lease, Some((ls, _)) if ls == nb.active_shard) {
                    drop(active_lease.take());
                    let lease = match &shard.parameters {
                        ShardParameters::F32(buffer) => {
                            ShardWriteLease::F32(owner.write(buffer.buffer())?)
                        }
                        ShardParameters::F16(words) => {
                            ShardWriteLease::F16(owner.write(words.buffer())?)
                        }
                    };
                    active_lease = Some((nb.active_shard, lease));
                }
                let job = &jobs[nb.job_idx];
                match (
                    active_lease.as_mut().map(|(_, lease)| lease),
                    job.controller().parameters(),
                ) {
                    (Some(ShardWriteLease::F32(lease)), Weights::F32(values)) => {
                        lease.write(offset, values)?;
                    }
                    (Some(ShardWriteLease::F16(lease)), Weights::F16(values)) => {
                        self.packed_f16.clear();
                        let word_count = values.len().div_ceil(2);
                        if self.packed_f16.capacity() < word_count {
                            reserve(&mut self.packed_f16, word_count, "metal binary16 words")?;
                        }
                        let (pairs, remainder) = values.as_chunks::<2>();
                        self.packed_f16.extend(pairs.iter().map(|[low, high]| {
                            u32::from(low.to_bits()) | (u32::from(high.to_bits()) << 16)
                        }));
                        if let Some(&last) = remainder.first() {
                            self.packed_f16.push(u32::from(last.to_bits()));
                        }
                        lease.write(offset / 2, &self.packed_f16)?;
                    }
                    _ => return Ok(None),
                }
                let slot = job.slot();
                shard.occupants[nb.local] = Some((slot.generation(), job.genome()));
                shard.live_count += 1;
                self.uploads += 1;
            }
            drop(active_lease);
        }

        // Compute per-shard totals, size each `shard.staged` once without
        // reallocation in steady state, and populate `touched` in ascending
        // `(topology, shard_idx)` order.
        let mut shard_totals = [0_usize; MAX_FAST_SHARDS];
        for counts in &chunk_counts {
            for s in 0..active_len {
                shard_totals[s] += counts[s];
            }
        }
        self.dispatched.clear();
        self.touched.clear();
        let touched_count = shard_totals[..active_len]
            .iter()
            .filter(|&&c| c > 0)
            .count();
        if self.touched.capacity() < touched_count {
            reserve(&mut self.touched, touched_count, "metal controller shards")?;
        }

        for s in 0..active_len {
            let (topology, shard_idx) = active_shards[s];
            let Some(shard) = self.buckets[topology]
                .as_mut()
                .and_then(|b| b.shards.dense.get_mut(shard_idx))
                .and_then(Option::as_mut)
            else {
                return Ok(None);
            };
            let count = shard_totals[s];
            if count == 0 {
                shard.staged.clear();
                continue;
            }
            self.touched.push((topology, shard_idx as u64));
            let needed_words = count.checked_mul(JOB_WORDS).ok_or(CoreError::Overflow {
                quantity: "metal jobs",
            })?;
            if shard.staged.capacity() < needed_words {
                let extra = needed_words.saturating_sub(shard.staged.len());
                reserve(&mut shard.staged, extra, "metal jobs")?;
            }
            if shard.staged.len() >= needed_words {
                shard.staged.truncate(needed_words);
            } else {
                shard.staged.resize(needed_words, 0);
            }
        }

        // Split every active shard's `staged` buffer into disjoint per-chunk
        // mutable slices so Pass B emits `JOB_WORDS` in exact ascending `row`
        // order across all workers with zero synchronization.
        let actual_chunks = chunk_counts.len();
        struct EmitChunk<'a, J> {
            base: usize,
            jobs: &'a [J],
            shard_outs: [&'a mut [u32]; MAX_FAST_SHARDS],
        }
        let mut emit_tasks: Vec<EmitChunk<'_, J>> = jobs
            .chunks(chunk_len)
            .enumerate()
            .map(|(idx, j_chunk)| EmitChunk {
                base: idx * chunk_len,
                jobs: j_chunk,
                shard_outs: std::array::from_fn(|_| <&mut [u32]>::default()),
            })
            .collect();

        let mut shard_slices: [&mut [u32]; MAX_FAST_SHARDS] =
            std::array::from_fn(|_| <&mut [u32]>::default());
        {
            let mut active_cursor = 0_usize;
            for bucket in self.buckets.iter_mut().flatten() {
                for shard in bucket.shards.dense.iter_mut().flatten() {
                    if active_cursor < active_len {
                        shard_slices[active_cursor] = shard.staged.as_mut_slice();
                        active_cursor += 1;
                    }
                }
            }
        }
        for (s, slice) in shard_slices.iter_mut().take(active_len).enumerate() {
            if shard_totals[s] == 0 {
                continue;
            }
            let mut rem = std::mem::take(slice);
            for t in 0..actual_chunks {
                let words = chunk_counts[t][s] * JOB_WORDS;
                let Some((head, tail)) = rem.split_at_mut_checked(words) else {
                    return Err(CoreError::LayoutMismatch {
                        buffer: "metal jobs",
                        detail: format!("shard {s} staging slice underrun for chunk {t}"),
                    });
                };
                rem = tail;
                emit_tasks[t].shard_outs[s] = head;
            }
        }

        pool::parallel_tasks(workers, "metal place stage", &mut emit_tasks, |_, task| {
            let mut cursors = [0_usize; MAX_FAST_SHARDS];
            for (local_i, job) in task.jobs.iter().enumerate() {
                let slot = job.slot();
                let topology = slot.topology() as usize;
                let Some(info) = topo_info[topology] else {
                    continue;
                };
                let raw_slot = slot.slot();
                let (shard_idx, offset_u64) = if raw_slot < info.slots_per_shard {
                    (0_usize, raw_slot * info.stride)
                } else {
                    let at = info.geometry.locate(raw_slot);
                    (at.shard as usize, at.offset)
                };
                let s = shard_map[topology][shard_idx] as usize;
                let pos = cursors[s];
                cursors[s] = pos + JOB_WORDS;
                let out = &mut task.shard_outs[s][pos..pos + JOB_WORDS];
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "jobs.len() <= u32::MAX and offset_u64 <= u32::MAX verified in Pass A"
                )]
                {
                    out[0] = (task.base + local_i) as u32;
                    out[1] = offset_u64 as u32;
                }
                out[2] = info.shape_words[0];
                out[3] = info.shape_words[1];
                out[4] = info.shape_words[2];
            }
        })?;

        Ok(Some((true, jobs.len())))
    }

    /// Grow the per-organism buffers to hold `count` organisms of
    /// `(input_width, output_width)` and allocate the 2-word `dims` uniform
    /// buffer on first use.
    fn hold(
        &mut self,
        count: usize,
        input_width: usize,
        output_width: usize,
    ) -> Result<(), CoreError> {
        let need_inputs = count.checked_mul(input_width).ok_or(CoreError::Overflow {
            quantity: "metal controller input elements",
        })?;
        let need_outputs = count.checked_mul(output_width).ok_or(CoreError::Overflow {
            quantity: "metal controller output elements",
        })?;
        if u32::try_from(need_inputs).is_err() || u32::try_from(need_outputs).is_err() {
            return Err(CoreError::Overflow {
                quantity: "metal controller staging index",
            });
        }
        if self.dims.is_none() {
            self.dims = Some(self.controllers.owner.zeroed_owned::<u32>(2)?);
            self.cached_dims = None;
        }
        if count <= self.fault_capacity
            && need_inputs <= self.input_capacity
            && need_outputs <= self.output_capacity
            && self.inputs.is_some()
            && self.raw.is_some()
            && self.faults.is_some()
        {
            return Ok(());
        }
        let organisms = count
            .checked_next_power_of_two()
            .ok_or(CoreError::Overflow {
                quantity: "metal controller staging organisms",
            })?
            .max(256);
        let in_elems = organisms
            .checked_mul(input_width)
            .ok_or(CoreError::Overflow {
                quantity: "metal controller input elements",
            })?
            .max(need_inputs);
        let out_elems = organisms
            .checked_mul(output_width)
            .ok_or(CoreError::Overflow {
                quantity: "metal controller output elements",
            })?
            .max(need_outputs);
        // Retire the old buffers first, so their bytes are free again, and
        // allocate all three before committing capacities so a mid-growth
        // error never leaves stale capacities behind.
        (self.inputs, self.raw, self.faults) = (None, None, None);
        self.input_capacity = 0;
        self.output_capacity = 0;
        self.fault_capacity = 0;
        let owner = &self.controllers.owner;
        let inputs = owner.zeroed_owned(in_elems)?;
        let raw = owner.zeroed_owned(out_elems)?;
        let faults = owner.zeroed_owned(organisms)?;
        self.inputs = Some(inputs);
        self.raw = Some(raw);
        self.faults = Some(faults);
        self.input_capacity = in_elems;
        self.output_capacity = out_elems;
        self.fault_capacity = organisms;
        Ok(())
    }
}

/// Narrow all `jobs`' sensor inputs (`f64 -> f32`) of width `input_width` into
/// `staged` and record per-job finiteness/width-validity in `finite` in a
/// single pass, parallelized over the persistent `flife_core::pool` worker
/// budget for large batches (zero per-tick OS thread spawns).
fn narrow_inputs(
    jobs: &[ControllerJob<'_>],
    input_width: usize,
    pool_workers: usize,
    staged: &mut Vec<f32>,
    finite: &mut Vec<u8>,
) -> Result<(), CoreError> {
    let total_inputs = jobs
        .len()
        .checked_mul(input_width)
        .ok_or(CoreError::Overflow {
            quantity: "metal inputs",
        })?;
    if staged.len() < total_inputs {
        reserve(staged, total_inputs - staged.len(), "metal inputs")?;
        staged.resize(total_inputs, 0.0);
    }
    if finite.len() < jobs.len() {
        reserve(finite, jobs.len() - finite.len(), "metal input flags")?;
        finite.resize(jobs.len(), 0);
    }
    let out = &mut staged[..total_inputs];
    let flags = &mut finite[..jobs.len()];
    let narrow_chunk = |jobs: &[ControllerJob<'_>], out: &mut [f32], flags: &mut [u8]| {
        for ((job, dst), flag) in jobs
            .iter()
            .zip(out.chunks_exact_mut(input_width))
            .zip(flags)
        {
            if job.inputs.len() != input_width {
                *flag = 0;
                continue;
            }
            let mut ok = true;
            for (slot, &value) in dst.iter_mut().zip(job.inputs) {
                let narrowed = value as f32;
                ok &= narrowed.is_finite();
                *slot = narrowed;
            }
            *flag = u8::from(ok);
        }
    };
    let chunks = pool_workers
        .clamp(1, PARALLEL_INPUT_MAX_THREADS)
        .min(jobs.len().div_ceil(1024));
    if jobs.len() < PARALLEL_INPUT_MIN_JOBS || chunks <= 1 {
        narrow_chunk(jobs, out, flags);
    } else {
        let chunk = jobs.len().div_ceil(chunks);
        let input_chunk = chunk.checked_mul(input_width).ok_or(CoreError::Overflow {
            quantity: "metal input chunk",
        })?;
        // Pass the stable `pool_workers` budget so all phases share ONE cached
        // private pool (`flife-w{W}-*`) rather than allocating a pool per chunk count.
        pool::scope_workers(pool_workers, "metal controller input staging", |scope| {
            let mut job_chunks = jobs.chunks(chunk);
            let mut out_chunks = out.chunks_mut(input_chunk);
            let mut flag_chunks = flags.chunks_mut(chunk);
            let head = (job_chunks.next(), out_chunks.next(), flag_chunks.next());
            for ((jc, oc), fc) in job_chunks.zip(out_chunks).zip(flag_chunks) {
                scope.spawn(move || narrow_chunk(jc, oc, fc));
            }
            if let (Some(jc), Some(oc), Some(fc)) = head {
                narrow_chunk(jc, oc, fc);
            }
        })?;
    }
    Ok(())
}

/// Unpack dense device output rows and fault flags into `outputs` in place
/// (reusing already-allocated `Some(RawOutputs)` slots across steady-state
/// ticks to avoid full-buffer clears and 328-byte struct overwrites),
/// parallelized over the persistent `flife_core::pool` for large batches.
fn unpack_outputs(
    values: &[f32],
    flags: &[u32],
    output_width: usize,
    pool_workers: usize,
    outputs: &mut Vec<Option<RawOutputs>>,
) -> Result<(), CoreError> {
    let count = flags.len();
    let expected_values = count.checked_mul(output_width).ok_or(CoreError::Overflow {
        quantity: "metal controller outputs",
    })?;
    if values.len() != expected_values {
        return Err(CoreError::LayoutMismatch {
            buffer: "metal controller outputs",
            detail: format!(
                "expected {expected_values} output values, got {}",
                values.len()
            ),
        });
    }
    let template = RawOutputs::zeroed(output_width)?;
    if outputs.len() > count {
        outputs.truncate(count);
    } else if outputs.len() < count {
        if outputs.capacity() < count {
            reserve(outputs, count - outputs.len(), "metal controller outputs")?;
        }
        outputs.resize(count, None);
    }
    let unpack_chunk = |rows: &[f32], flag_slice: &[u32], dst: &mut [Option<RawOutputs>]| {
        for ((row, &flag), slot) in rows.chunks_exact(output_width).zip(flag_slice).zip(dst) {
            if flag == 0 {
                match slot {
                    Some(existing) if existing.len() == output_width => {
                        existing.as_mut_slice().copy_from_slice(row);
                    }
                    _ => {
                        let mut out = template;
                        out.as_mut_slice().copy_from_slice(row);
                        *slot = Some(out);
                    }
                }
            } else {
                *slot = None;
            }
        }
    };
    let chunks = pool_workers
        .clamp(1, PARALLEL_INPUT_MAX_THREADS)
        .min(count.div_ceil(1024));
    if count < PARALLEL_INPUT_MIN_JOBS || chunks <= 1 {
        unpack_chunk(values, flags, outputs.as_mut_slice());
    } else {
        let chunk = count.div_ceil(chunks);
        let row_chunk = chunk.checked_mul(output_width).ok_or(CoreError::Overflow {
            quantity: "metal output chunk",
        })?;
        let dst = outputs.as_mut_slice();
        pool::scope_workers(pool_workers, "metal controller output unpack", |scope| {
            let mut row_chunks = values.chunks(row_chunk);
            let mut flag_chunks = flags.chunks(chunk);
            let mut dst_chunks = dst.chunks_mut(chunk);
            let head = (row_chunks.next(), flag_chunks.next(), dst_chunks.next());
            for ((rc, fc), dc) in row_chunks.zip(flag_chunks).zip(dst_chunks) {
                scope.spawn(move || unpack_chunk(rc, fc, dc));
            }
            if let (Some(rc), Some(fc), Some(dc)) = head {
                unpack_chunk(rc, fc, dc);
            }
        })?;
    }
    Ok(())
}

impl MetalBackend {
    /// Upload per-shard job words, update `[input_width, output_width]` when
    /// changed, upload `count * input_width` F32 inputs directly from either
    /// `external_inputs` (zero-copy flat-F32 fast path) or `self.staged_inputs`,
    /// and submit all touched shards in one Metal command buffer.
    #[expect(
        clippy::too_many_lines,
        reason = "shared shard upload and inline/heap-spill Metal batch dispatch"
    )]
    fn dispatch_staged(
        &mut self,
        count: usize,
        input_width: usize,
        output_width: usize,
        external_inputs: Option<&[f32]>,
    ) -> Result<(), CoreError> {
        self.hold(count, input_width, output_width)?;
        let owner = self.controllers.owner.clone();
        let touched_len = self.touched.len();
        for idx in 0..touched_len {
            let (topology, key) = self.touched[idx];
            let Some(shard) = self
                .buckets
                .get_mut(topology)
                .and_then(Option::as_mut)
                .and_then(|bucket| bucket.shards.get_mut(key))
            else {
                continue;
            };
            let needed = shard.staged.len() / JOB_WORDS;
            if shard.jobs.is_none() || shard.job_capacity < needed {
                let capacity = needed
                    .checked_next_power_of_two()
                    .ok_or(CoreError::Overflow {
                        quantity: "metal shard jobs",
                    })?
                    .max(64);
                let words = capacity.checked_mul(JOB_WORDS).ok_or(CoreError::Overflow {
                    quantity: "metal shard job words",
                })?;
                shard.jobs = None;
                shard.job_capacity = 0;
                shard.jobs = Some(owner.zeroed_owned(words)?);
                shard.job_capacity = capacity;
            }
            if let Some(words) = shard.jobs.as_ref() {
                owner.write_slice(words.buffer(), 0, &shard.staged)?;
            }
        }
        let (Some(inputs), Some(raw), Some(faults), Some(dims)) = (
            self.inputs.as_ref().map(Owned::buffer),
            self.raw.as_ref().map(Owned::buffer),
            self.faults.as_ref().map(Owned::buffer),
            self.dims.as_ref().map(Owned::buffer),
        ) else {
            return Err(CoreError::StateConflict {
                operation: "metal controller residency",
                detail: "buffers missing".to_owned(),
            });
        };
        let dims_words = [
            u32::try_from(input_width).map_err(|_| CoreError::Overflow {
                quantity: "metal controller input width",
            })?,
            u32::try_from(output_width).map_err(|_| CoreError::Overflow {
                quantity: "metal controller output width",
            })?,
        ];
        if self.cached_dims != Some(dims_words) {
            owner.write_slice(dims, 0, &dims_words)?;
            self.cached_dims = Some(dims_words);
        }
        let input_slice = match external_inputs {
            Some(ext) => &ext[..count * input_width],
            None => &self.staged_inputs[..count * input_width],
        };
        owner.write_slice(inputs, 0, input_slice)?;
        let receipt = if touched_len <= INLINE_SHARDS {
            type InlineShardSpec<'a> = (&'a Pipeline, [Binding; 6], usize, usize);
            let mut inline_specs: [Option<InlineShardSpec<'_>>; INLINE_SHARDS] =
                [const { None }; INLINE_SHARDS];
            for (slot, &(topology, key)) in inline_specs.iter_mut().zip(self.touched.iter()) {
                let bucket = self.buckets.get(topology).and_then(Option::as_ref);
                let shard = bucket.and_then(|bucket| bucket.shards.get(key));
                let Some((dtype, threads, parameters, words, staged)) =
                    bucket.zip(shard).and_then(|(bucket, shard)| {
                        Some((
                            bucket.dtype,
                            forward_threads(bucket.widest_hidden),
                            shard.parameters.reads(),
                            shard.jobs.as_ref()?.buffer(),
                            shard.staged.len() / JOB_WORDS,
                        ))
                    })
                else {
                    return Err(CoreError::StateConflict {
                        operation: "metal controller residency",
                        detail: format!("shard {key} of member {topology} is missing"),
                    });
                };
                let pipeline = match dtype {
                    WeightDtype::F32 => &self.controllers.forward,
                    WeightDtype::F16 => &self.controllers.forward_half,
                };
                *slot = Some((
                    pipeline,
                    [
                        parameters,
                        inputs.reads(),
                        words.reads(),
                        raw.writes(),
                        faults.writes(),
                        dims.reads(),
                    ],
                    staged,
                    threads,
                ));
            }
            let mut kernels = [Kernel {
                pipeline: &self.controllers.forward,
                bindings: &[],
                grid: Grid::linear(1, 1),
            }; INLINE_SHARDS];
            for (dst, spec) in kernels
                .iter_mut()
                .zip(inline_specs.iter().take(touched_len))
            {
                if let Some((pipeline, bound, staged, threads)) = spec.as_ref() {
                    *dst = Kernel {
                        pipeline,
                        bindings: bound,
                        grid: Grid::linear(*staged, *threads),
                    };
                }
            }
            owner.submit_batch(&kernels[..touched_len])?
        } else {
            let mut bindings = Vec::new();
            reserve(&mut bindings, touched_len, "metal controller kernels")?;
            for &(topology, key) in &self.touched {
                let bucket = self.buckets.get(topology).and_then(Option::as_ref);
                let shard = bucket.and_then(|bucket| bucket.shards.get(key));
                let Some((dtype, threads, parameters, words, staged)) =
                    bucket.zip(shard).and_then(|(bucket, shard)| {
                        Some((
                            bucket.dtype,
                            forward_threads(bucket.widest_hidden),
                            shard.parameters.reads(),
                            shard.jobs.as_ref()?.buffer(),
                            shard.staged.len() / JOB_WORDS,
                        ))
                    })
                else {
                    return Err(CoreError::StateConflict {
                        operation: "metal controller residency",
                        detail: format!("shard {key} of member {topology} is missing"),
                    });
                };
                let pipeline = match dtype {
                    WeightDtype::F32 => &self.controllers.forward,
                    WeightDtype::F16 => &self.controllers.forward_half,
                };
                bindings.push((
                    pipeline,
                    [
                        parameters,
                        inputs.reads(),
                        words.reads(),
                        raw.writes(),
                        faults.writes(),
                        dims.reads(),
                    ],
                    staged,
                    threads,
                ));
            }
            let kernels: Vec<Kernel<'_>> = bindings
                .iter()
                .map(|(pipeline, bound, staged, threads)| Kernel {
                    pipeline,
                    bindings: bound,
                    grid: Grid::linear(*staged, *threads),
                })
                .collect();
            owner.submit_batch(&kernels)?
        };
        self.last_command = Some((receipt.kernels, receipt.device_seconds));
        Ok(())
    }

    /// Read `count * output_width` raw outputs directly into `external_values`
    /// (zero-copy flat-F32 fast path) or `self.values`, and `count` fault words
    /// into `self.flags`.
    fn read_back_results(
        &mut self,
        count: usize,
        output_width: usize,
        external_values: Option<&mut [f32]>,
    ) -> Result<(), CoreError> {
        let (Some(raw), Some(faults)) = (
            self.raw.as_ref().map(Owned::buffer),
            self.faults.as_ref().map(Owned::buffer),
        ) else {
            return Err(CoreError::StateConflict {
                operation: "metal controller residency",
                detail: "buffers missing".to_owned(),
            });
        };
        let need_values = count * output_width;
        match external_values {
            Some(dst) => {
                self.controllers
                    .owner
                    .read_back(raw, &mut dst[..need_values])?;
            }
            None => {
                if self.values.len() < need_values {
                    let extra = need_values - self.values.len();
                    reserve(&mut self.values, extra, "metal controller outputs")?;
                    self.values.resize(need_values, 0.0);
                }
                self.controllers
                    .owner
                    .read_back(raw, &mut self.values[..need_values])?;
            }
        }
        if self.flags.len() < count {
            let extra = count - self.flags.len();
            reserve(&mut self.flags, extra, "metal controller faults")?;
            self.flags.resize(count, 0);
        }
        self.controllers
            .owner
            .read_back(faults, &mut self.flags[..count])
    }

    fn evaluate_inner(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<RawOutputs>>,
    ) -> Result<(), CoreError> {
        self.last_command = None;
        if jobs.is_empty() {
            outputs.clear();
            return Ok(());
        }
        let Some((input_width, output_width)) = jobs.iter().find_map(|job| {
            let topo = job.controller.topology();
            let len = job.controller.parameters().len();
            (job.inputs.len() == topo.input_width() && job_words(topo, len, 0, 0).is_ok())
                .then_some((topo.input_width(), topo.raw_output_width()))
        }) else {
            outputs.clear();
            reserve(outputs, jobs.len(), "metal controller outputs")?;
            outputs.resize(jobs.len(), None);
            return Ok(());
        };
        narrow_inputs(
            jobs,
            input_width,
            self.staging_workers,
            &mut self.staged_inputs,
            &mut self.input_finite,
        )?;
        let (all_dispatched, count) =
            self.place_and_stage(jobs, None, (input_width, output_width))?;
        if count > 0 {
            if !all_dispatched {
                for (row, &index) in self.dispatched.iter().enumerate() {
                    if row != index {
                        let src = index * input_width;
                        self.staged_inputs
                            .copy_within(src..src + input_width, row * input_width);
                    }
                }
            }
            self.dispatch_staged(count, input_width, output_width, None)?;
            self.read_back_results(count, output_width, None)?;
        }
        if count == jobs.len() {
            unpack_outputs(
                &self.values[..count * output_width],
                &self.flags[..count],
                output_width,
                self.staging_workers,
                outputs,
            )?;
        } else {
            outputs.clear();
            if outputs.capacity() < jobs.len() {
                reserve(outputs, jobs.len(), "metal controller outputs")?;
            }
            let template = RawOutputs::zeroed(output_width)?;
            let mut dispatched = self.dispatched.iter().zip(
                self.values[..count * output_width]
                    .chunks_exact(output_width)
                    .zip(&self.flags[..count]),
            );
            let mut next = dispatched.next();
            for index in 0..jobs.len() {
                match next {
                    Some((&at, (row, &flag))) if at == index => {
                        outputs.push(if flag == 0 {
                            let mut out = template;
                            out.as_mut_slice().copy_from_slice(row);
                            Some(out)
                        } else {
                            None
                        });
                        next = dispatched.next();
                    }
                    _ => outputs.push(None),
                }
            }
        }
        Ok(())
    }

    fn evaluate_flat_f32_inner(
        &mut self,
        jobs: &[FlatControllerJob<'_>],
        input_width: usize,
        inputs_f32: &[f32],
        inputs_finite: &[u8],
        outputs: &mut FlatOutputs,
    ) -> Result<(), CoreError> {
        self.last_command = None;
        let expected_inputs = jobs
            .len()
            .checked_mul(input_width)
            .ok_or(CoreError::Overflow {
                quantity: "flat controller inputs",
            })?;
        if inputs_f32.len() != expected_inputs {
            return Err(CoreError::LayoutMismatch {
                buffer: "flat controller inputs",
                detail: format!(
                    "expected {expected_inputs} f32 inputs ({} jobs x {input_width}), got {}",
                    jobs.len(),
                    inputs_f32.len()
                ),
            });
        }
        if inputs_finite.len() != jobs.len() {
            return Err(CoreError::LayoutMismatch {
                buffer: "flat controller input flags",
                detail: format!(
                    "expected {} finite flags, got {}",
                    jobs.len(),
                    inputs_finite.len()
                ),
            });
        }
        let Some(first) = jobs.first() else {
            outputs.clear();
            return Ok(());
        };
        let Some(output_width) = jobs.iter().find_map(|job| {
            let topo = job.controller.topology();
            let len = job.controller.parameters().len();
            (topo.input_width() == input_width && job_words(topo, len, 0, 0).is_ok())
                .then_some(topo.raw_output_width())
        }) else {
            let fallback_width = first.controller.topology().raw_output_width();
            outputs.prepare(jobs.len(), fallback_width)?;
            return Ok(());
        };
        let (all_dispatched, count) =
            self.place_and_stage(jobs, Some(inputs_finite), (input_width, output_width))?;
        outputs.prepare(jobs.len(), output_width)?;
        if count == 0 {
            return Ok(());
        }
        if all_dispatched && count == jobs.len() {
            // Fast path: write `inputs_f32` directly into the Metal shared
            // input buffer and read `raw` directly into `outputs.values` with
            // zero intermediate host copies and zero worker-pool wakeups.
            self.dispatch_staged(count, input_width, output_width, Some(inputs_f32))?;
            self.read_back_results(count, output_width, Some(&mut outputs.values))?;
            for (dst, &flag) in outputs.valid[..count].iter_mut().zip(&self.flags[..count]) {
                *dst = u8::from(flag == 0);
            }
        } else {
            let total_staged = count.checked_mul(input_width).ok_or(CoreError::Overflow {
                quantity: "metal inputs",
            })?;
            if self.staged_inputs.len() < total_staged {
                let extra = total_staged - self.staged_inputs.len();
                reserve(&mut self.staged_inputs, extra, "metal inputs")?;
                self.staged_inputs.resize(total_staged, 0.0);
            }
            for (row, &index) in self.dispatched.iter().enumerate() {
                let src = index * input_width;
                let dst = row * input_width;
                self.staged_inputs[dst..dst + input_width]
                    .copy_from_slice(&inputs_f32[src..src + input_width]);
            }
            self.dispatch_staged(count, input_width, output_width, None)?;
            self.read_back_results(count, output_width, None)?;
            for (&index, (row, &flag)) in self.dispatched.iter().zip(
                self.values[..count * output_width]
                    .chunks_exact(output_width)
                    .zip(&self.flags[..count]),
            ) {
                if flag == 0 {
                    let dst = index * output_width;
                    outputs.values[dst..dst + output_width].copy_from_slice(row);
                    outputs.valid[index] = 1;
                }
            }
        }
        Ok(())
    }
}

impl ControllerBackend for MetalBackend {
    fn describe(&self) -> String {
        format!("metal ({})", self.controllers.device_name())
    }

    fn last_device_nanoseconds(&self) -> u64 {
        self.last_command
            .map_or(0, |(_, seconds)| owner::nanoseconds(seconds))
    }

    fn resident_bytes(&self) -> u64 {
        let shards = self
            .buckets
            .iter()
            .flatten()
            .flat_map(|bucket| bucket.shards.values())
            .map(|shard| {
                capacity_bytes(&shard.occupants)
                    .saturating_add(capacity_bytes(&shard.staged))
                    .saturating_add(SHARD_HOST_BYTES)
            });
        total(shards.chain([
            self.controllers.owner.usage().bytes,
            capacity_bytes(&self.staged_inputs),
            capacity_bytes(&self.input_finite),
            capacity_bytes(&self.dispatched),
            capacity_bytes(&self.touched),
            capacity_bytes(&self.packed_f16),
            capacity_bytes(&self.values),
            capacity_bytes(&self.flags),
        ]))
    }

    fn forget(&mut self) {
        self.buckets.clear();
        self.touched.clear();
        self.packed_f16.clear();
        self.shard_count = 0;
        self.held = 0;
        self.uploads = 0;
    }

    fn release_slots(&mut self, slots: &[ArenaSlot]) {
        for &slot in slots {
            let Ok(topology) = usize::try_from(slot.topology()) else {
                continue;
            };
            let Some(bucket) = self.buckets.get_mut(topology).and_then(Option::as_mut) else {
                continue;
            };
            let Some(geometry) = bucket.geometry else {
                continue;
            };
            let at = geometry.locate(slot.slot());
            let Ok(local) = usize::try_from(at.slot_in_shard) else {
                continue;
            };
            let drop_shard = match bucket.shards.get_mut(at.shard) {
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
            if drop_shard && let Some(removed) = bucket.shards.remove(at.shard) {
                self.held = self.held.saturating_sub(removed.bytes);
                self.shard_count = self.shard_count.saturating_sub(1);
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
        let result = self.evaluate_inner(jobs, outputs);
        if result.is_err() {
            outputs.clear();
        }
        result
    }
}

/// Which controller evaluator a run uses. No result depends on the choice;
/// it decides only where the arithmetic runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ControllerChoice {
    /// The CPU reference alone.
    Cpu,
    /// FrankenTorch's serial population kernel on the world's CPU workers
    /// ([`CpuBackend`], FL-025), on any host. Bit-identical to the
    /// reference, and it defers nothing the reference evaluates.
    Torch,
    /// The Metal backend; refused on a host without a Metal device.
    Metal,
    /// Metal where this host has a Metal device and the world runs on at
    /// most [`AUTO_METAL_MAX_WORKERS`] CPU workers; otherwise
    /// [`ControllerChoice::Torch`] on at most [`AUTO_TORCH_MAX_WORKERS`], or,
    /// where [`TORCH_AT_SCALE`], for a world of at least
    /// [`AUTO_TORCH_MIN_ORGANISMS`]; otherwise the CPU reference.
    #[default]
    Auto,
}

/// The most CPU workers at which [`ControllerChoice::Auto`] takes Metal.
/// Measured on an Apple M5 Ultra: Metal beats CPU/Torch at 1–4 workers
/// (2.2x on 1 worker, 1.7x on 2, 1.3x on 4 on the preset), whereas with the
/// flat-F32 `ControllerBackend::evaluate_flat_f32` path (`phase_profile`,
/// bit-exact digests, 2026-11-23), [`ControllerChoice::Torch`] beats Metal at
/// 8, 12 and 16 workers across every measured workload — Bundle A (~11k, 32
/// ticks): `torch 28.87 / 25.17 / 23.76 ms/t` vs `metal 30.36 / 27.14 /
/// 28.26 ms/t`; Bundle B (~18k, 32 ticks): `torch 31.73 / 27.74 / 27.49 ms/t`
/// vs `metal 34.15 / 32.12 / 31.63 ms/t`; Confluent 65k (65,536 -> 69,463,
/// F16, 8 ticks): `torch 152.42 / 132.64 / 120.51 ms/t` vs `metal 176.95 /
/// 154.39 / 160.56 ms/t`.
pub const AUTO_METAL_MAX_WORKERS: usize = 4;

/// The most CPU workers at which [`ControllerChoice::Auto`] takes
/// FrankenTorch's CPU route where it does not take Metal (FL-025). With
/// `ControllerBackend::evaluate_flat_f32` and the persistent private worker
/// pool, measured on an Apple M5 Ultra (`phase_profile`, bit-exact digests,
/// 2026-11-23), `Torch` beats the scalar CPU reference across 8, 12 and 16
/// workers on every measured workload — Bundle A: `28.87 / 25.17 / 23.76 ms/t`
/// vs `cpu 35.92 / 28.79 / 27.15 ms/t` (1.24x / 1.14x / 1.14x); Bundle B:
/// `31.73 / 27.74 / 27.49 ms/t` vs `cpu 44.91 / 35.53 / 33.03 ms/t` (1.42x /
/// 1.28x / 1.20x); Confluent 65k: `152.42 / 132.64 / 120.51 ms/t` vs
/// `cpu 181.82 / 147.12 / 140.03 ms/t` (1.19x / 1.11x / 1.16x) — as well as
/// 1–4 workers on both macOS (1.62x / 1.26x) and Linux x86-64 (1.41x / 1.11x).
pub const AUTO_TORCH_MAX_WORKERS: usize = 64;

/// The fewest organisms at which [`ControllerChoice::Auto`] takes
/// FrankenTorch's CPU route on any number of CPU workers where it does not
/// take Metal, on a host where [`TORCH_AT_SCALE`]. There the controllers
/// are most of the tick.
///
/// Measured on sixteen workers with `tick_throughput`, 2026-11-15:
/// - Ticks 32 to 160 of a founding world with every founder alive, each
///   16-tick block against the reference in the same process, the same
///   digests, two invocations each.
/// - On an M5 Ultra: 1.030x and 1.022x at 25,000 (the Interactive preset)
///   and 1.152x and 1.092x at 100,000 (the Desktop ecology preset), with at
///   least 7 of 8 blocks faster in each and every cv at most 2.4%.
/// - On Linux x86-64 (an AMD Threadripper PRO 5975WX): 0.76x and 0.88x at
///   25,000 and 0.80x and 0.80x at 100,000, with no block faster and every
///   cv at most 1.9%. So the floor holds only where it was measured faster.
pub const AUTO_TORCH_MIN_ORGANISMS: usize = 25_000;

/// Whether [`AUTO_TORCH_MIN_ORGANISMS`] applies on this build's host: on
/// Apple silicon under macOS, where FrankenTorch's CPU route measured faster
/// than the reference at scale on any number of workers. Its kernel is a
/// CPU kernel, so this is the CPU's property, not the Metal device's. On
/// Linux x86-64 it measured slower there, and other hosts are unmeasured.
pub const TORCH_AT_SCALE: bool = cfg!(all(target_os = "macos", target_arch = "aarch64"));

impl ControllerChoice {
    /// What [`ControllerChoice::Auto`] is for a world of `organisms` on
    /// `cpu_workers` threads on this build's kind of host ([`TORCH_AT_SCALE`]),
    /// with or without a Metal device: [`ControllerChoice::auto_on`].
    #[must_use]
    pub const fn auto_for(cpu_workers: usize, metal_device: bool, organisms: usize) -> Self {
        Self::auto_on(cpu_workers, metal_device, TORCH_AT_SCALE, organisms)
    }

    /// The rule on any kind of host: Metal up to [`AUTO_METAL_MAX_WORKERS`]
    /// where there is a device; otherwise `torch` up to
    /// [`AUTO_TORCH_MAX_WORKERS`], or from [`AUTO_TORCH_MIN_ORGANISMS`] where
    /// `torch_at_scale`; otherwise the CPU reference.
    #[must_use]
    pub const fn auto_on(
        cpu_workers: usize,
        metal_device: bool,
        torch_at_scale: bool,
        organisms: usize,
    ) -> Self {
        if metal_device && cpu_workers <= AUTO_METAL_MAX_WORKERS {
            Self::Metal
        } else if cpu_workers <= AUTO_TORCH_MAX_WORKERS
            || (torch_at_scale && organisms >= AUTO_TORCH_MIN_ORGANISMS)
        {
            Self::Torch
        } else {
            Self::Cpu
        }
    }

    /// The backend this choice selects on this host for a world of
    /// `organisms` on `cpu_workers` threads, or `None` for the CPU
    /// reference.
    ///
    /// # Errors
    /// For [`ControllerChoice::Metal`], [`MetalBackend::open`]'s errors. For
    /// [`ControllerChoice::Auto`], those other than a missing Metal device:
    /// a Mac whose kernel does not compile is a defect to report, not a
    /// reason to fall back quietly.
    ///
    /// The backend holds at most `budget` (FL-043): see
    /// [`CpuBackend::with_budget`] and [`MetalBackend::with_budget`].
    pub fn backend(
        self,
        cpu_workers: usize,
        organisms: usize,
        budget: BackendBudget,
    ) -> Result<Option<Box<dyn ControllerBackend>>, CoreError> {
        let boxed = |backend: MetalBackend| -> Box<dyn ControllerBackend> { Box::new(backend) };
        let metal = || -> Result<MetalBackend, CoreError> {
            let mut backend = MetalBackend::with_budget(u64::MAX, budget)?;
            if let Some(workers) = NonZeroUsize::new(cpu_workers) {
                backend.set_staging_workers(workers);
            }
            Ok(backend)
        };
        match self {
            Self::Cpu => Ok(None),
            Self::Torch => {
                let lanes = NonZeroUsize::new(cpu_workers).unwrap_or(NonZeroUsize::MIN);
                Ok(Some(Box::new(CpuBackend::with_budget(
                    lanes,
                    flife_backend_cpu::SHARD_SLOTS,
                    budget,
                ))))
            }
            Self::Metal => metal().map(boxed).map(Some),
            Self::Auto => {
                // Opening the device is how this host says whether it has
                // one, so only where `auto_for` could choose it.
                if Self::auto_for(cpu_workers, true, organisms) == Self::Metal {
                    match metal() {
                        Ok(backend) => return Ok(Some(boxed(backend))),
                        Err(CoreError::MissingCapability { .. }) => {}
                        Err(error) => return Err(error),
                    }
                }
                Self::auto_for(cpu_workers, false, organisms).backend(
                    cpu_workers,
                    organisms,
                    budget,
                )
            }
        }
    }
}

impl std::str::FromStr for ControllerChoice {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "cpu" => Ok(Self::Cpu),
            "torch" => Ok(Self::Torch),
            "metal" => Ok(Self::Metal),
            "auto" => Ok(Self::Auto),
            other => Err(format!(
                "`{other}` is not `cpu`, `torch`, `metal` or `auto`"
            )),
        }
    }
}

#[cfg(test)]
mod staging_tests {
    use flife_core::ids::GenomeId;
    use flife_neural::{ArenaSlot, ControllerJob, DenseTopology, RawOutputs, ReferenceController};

    use super::{narrow_inputs, unpack_outputs};

    #[test]
    fn pool_narrow_inputs_and_in_place_unpack_outputs_match_serial_across_shapes() {
        let count = 3_000_usize;
        let in_w = 108_usize;
        let out_w = 44_usize;
        let topo = DenseTopology::with_io(in_w as u32, &[8], out_w as u32).expect("topology");
        let params = vec![0.0_f32; topo.parameter_count()];
        let ctrl = ReferenceController::new(&topo, &params, 4.0).expect("controller");

        let inputs: Vec<Vec<f64>> = (0..count)
            .map(|i| {
                let len = if i == 17 { in_w - 1 } else { in_w };
                (0..len)
                    .map(|j| {
                        if i == 2_500 && j == 3 {
                            f64::INFINITY
                        } else {
                            ((i as f64) * 0.013 - (j as f64) * 0.07).sin()
                        }
                    })
                    .collect()
            })
            .collect();
        let jobs: Vec<ControllerJob<'_>> = (0..count)
            .map(|i| ControllerJob {
                genome: GenomeId::new(i as u64 + 1),
                slot: ArenaSlot::new(0, i as u64, 1),
                controller: ctrl,
                inputs: &inputs[i],
            })
            .collect();

        let (mut serial_staged, mut serial_finite) = (Vec::new(), Vec::new());
        narrow_inputs(&jobs, in_w, 1, &mut serial_staged, &mut serial_finite).expect("serial");
        assert_eq!(serial_finite[17], 0, "short input rejected");
        assert_eq!(serial_finite[2_500], 0, "non-finite input rejected");
        assert_eq!(serial_finite[0], 1);

        for workers in [4_usize, 8] {
            let (mut par_staged, mut par_finite) = (Vec::new(), Vec::new());
            narrow_inputs(&jobs, in_w, workers, &mut par_staged, &mut par_finite)
                .expect("parallel");
            assert_eq!(par_finite, serial_finite);
            for (i, &ok) in serial_finite.iter().enumerate() {
                if ok == 1 {
                    let s = &serial_staged[i * in_w..(i + 1) * in_w];
                    let p = &par_staged[i * in_w..(i + 1) * in_w];
                    assert_eq!(
                        p.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                        s.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                        "row {i} workers={workers}"
                    );
                }
            }
        }

        // Unpack outputs: test first fill, in-place steady-state overwrite with
        // new deferrals + restored slots, shrink, grow, and width change.
        let mut values: Vec<f32> = (0..count * out_w)
            .map(|k| ((k % 97) as f32) * 0.125 - 6.0)
            .collect();
        let mut flags = vec![0_u32; count];
        flags[13] = 1;
        flags[2_999] = 2;

        let mut serial_out = Vec::new();
        unpack_outputs(&values, &flags, out_w, 1, &mut serial_out).expect("serial unpack");

        let mut reused_out = Vec::new();
        unpack_outputs(&values, &flags, out_w, 8, &mut reused_out).expect("parallel unpack 1");
        assert_eq!(reused_out, serial_out);

        // Overwrite in place: flip values, clear old deferrals, defer a previously-Some slot.
        for v in &mut values {
            *v = -*v + 0.5;
        }
        flags[13] = 0;
        flags[2_999] = 0;
        flags[500] = 1;
        unpack_outputs(&values, &flags, out_w, 1, &mut serial_out).expect("serial unpack 2");
        unpack_outputs(&values, &flags, out_w, 8, &mut reused_out).expect("parallel unpack 2");
        assert_eq!(reused_out, serial_out);
        assert!(reused_out[500].is_none());
        assert!(reused_out[13].is_some());

        // Shrink then grow across width change (44 -> 36).
        let shrink_n = 2_100_usize;
        let out_w2 = 36_usize;
        let values2: Vec<f32> = (0..shrink_n * out_w2).map(|k| (k as f32) * 0.01).collect();
        let flags2 = vec![0_u32; shrink_n];
        let mut fresh2 = Vec::new();
        unpack_outputs(&values2, &flags2, out_w2, 1, &mut fresh2).expect("fresh w2");
        unpack_outputs(&values2, &flags2, out_w2, 4, &mut reused_out).expect("reused w2");
        assert_eq!(reused_out.len(), shrink_n);
        assert_eq!(reused_out, fresh2);
        assert!(
            reused_out
                .iter()
                .all(|o| o.is_some_and(|raw: RawOutputs| raw.len() == out_w2))
        );
    }
}
