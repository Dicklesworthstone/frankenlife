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

use flife_backend_cpu::CpuBackend;
use flife_core::CoreError;
use flife_core::ids::GenomeId;
use flife_core::memory::{bytes_for, capacity_bytes, total};
use flife_neural::{
    ArenaSlot, ControllerBackend, ControllerJob, DenseTopology, MAX_CONTROLLER_OUTPUT_WIDTH,
    MAX_SENSOR_WIDTH, RawOutputs, ShardGeometry, WeightDtype, Weights,
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
    MaintenanceOutcome, MotionConstants, MotionOrganism, MotionOutcome, MovementOutcome,
    MutationBirth, MutationConstants, MutationCounts, MutationLayer, MutationValues,
    ProposalConstants, ProposalOrganism, ProposalOutcome,
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

/// Threads per organism in the forward dispatch.
pub const FORWARD_THREADS: usize = 64;

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
        let owner = DeviceOwner::open(limits)?;
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
/// the preset), so a shard is large: 6,718 genomes of the preset's
/// 9,988-parameter topology. A topology's first genome allocates a whole
/// shard.
pub const SHARD_BYTES: u64 = 256 << 20;

/// Under a budget, a shard holds at most this fraction of what a full
/// batch's staging leaves of it (FL-043): room for the five catalog
/// members' first shards and three more.
pub const SHARD_SHARE: u64 = 8;

/// The host bytes one shard holds beside its occupants: its job list and
/// its places in a batch's touched shards and kernel bindings.
const SHARD_HOST_BYTES: u64 = 256;

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

/// One shard of a topology's slots on the device: a whole shard's buffer,
/// made when its first slot is needed, kept until every occupant retires
/// (`release_slots`, FL-043) or `forget` drops it.
struct DeviceShard {
    parameters: ShardParameters,
    /// Each slot's uploaded occupant, its generation and genome, or `None`
    /// before its first or after retirement.
    occupants: Vec<Option<(u32, GenomeId)>>,
    /// Live uploaded occupants in `occupants`.
    live_count: usize,
    /// Exact bytes charged to `MetalBackend::held` when this shard was created.
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
    /// Validated `[hidden_layers, w0, w1]` words for the kernel's `jobs`
    /// buffer, cached once per topology; `None` for a shape the kernel refuses.
    shape_words: Option<[u32; 3]>,
    /// `None` when not one slot fits a device shard: its jobs are deferred.
    geometry: Option<ShardGeometry>,
    shards: ShardTable,
}

/// Minimum batch size before parallelizing host `f64 -> f32` input narrowing.
const PARALLEL_INPUT_MIN_JOBS: usize = 2_048;

/// Maximum host threads used for narrowing sensor inputs into `staged_inputs`.
const PARALLEL_INPUT_MAX_THREADS: usize = 8;

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
    /// Host staging, reused.
    staged_inputs: Vec<f32>,
    input_finite: Vec<u8>,
    dispatched: Vec<usize>,
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
        let controllers = MetalControllers::open_with(Limits {
            bytes: budget.bytes,
            leases_per_purpose: u32::MAX,
        })?;
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
            staged_inputs: Vec::new(),
            input_finite: Vec::new(),
            dispatched: Vec::new(),
            values: Vec::new(),
            flags: Vec::new(),
            uploads: 0,
            last_command: None,
        })
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

    /// The bucket serving `job`, made on its topology's first job; `None`
    /// for a job it defers.
    fn bucket(&mut self, job: &ControllerJob<'_>) -> Result<Option<&mut DeviceBucket>, CoreError> {
        let Ok(index) = usize::try_from(job.slot.topology()) else {
            return Ok(None);
        };
        if self.buckets.len() <= index {
            let more = index + 1 - self.buckets.len();
            reserve(&mut self.buckets, more, "metal controller buckets")?;
            self.buckets.resize_with(index + 1, || None);
        }
        let topology = job.controller.topology();
        let length = job.controller.parameters().len() as u64;
        let dtype = job.controller.parameters().dtype();
        let (owner, shard_slots) = (&self.controllers.owner, self.shard_slots);
        let share = match self.budget.bytes {
            u64::MAX => u64::MAX,
            bytes => bytes.saturating_sub(staging_bytes(self.budget.organisms, 0)) / SHARD_SHARE,
        };
        let bucket = self.buckets[index].get_or_insert_with(|| {
            // A 32-bit job word offsets every element of a shard.
            let limit = (owner.max_buffer_bytes() as u64)
                .min(SHARD_BYTES)
                .min(share)
                / dtype.bytes();
            let shape_words = usize::try_from(length)
                .ok()
                .and_then(|len| job_words(topology, len, 0, 0).ok())
                .map(|words| [words[2], words[3], words[4]]);
            // Trap A defense-in-depth: `fitting_for_dtype` pads F16 slot stride
            // to a multiple of 4 elements, guaranteeing zero F16 alignment
            // deferrals even for odd-parameter topologies.
            DeviceBucket {
                topology: topology.clone(),
                dtype,
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
    #[expect(
        clippy::too_many_lines,
        reason = "single-pass placement and staging with duplicate-slot fallback"
    )]
    fn place_and_stage(
        &mut self,
        jobs: &[ControllerJob<'_>],
        batch_dims: (usize, usize),
    ) -> Result<(Vec<(usize, u64)>, bool), CoreError> {
        let owner = self.controllers.owner.clone();
        let mut uploads = 0;
        let budget = self.budget;
        let (mut held, mut shard_count) = (self.held, self.shard_count);
        let mut packed: Vec<u32> = Vec::new();
        for bucket in self.buckets.iter_mut().flatten() {
            for shard in bucket.shards.values_mut() {
                shard.staged.clear();
            }
        }
        let mut dispatched = std::mem::take(&mut self.dispatched);
        dispatched.clear();
        if dispatched.capacity() < jobs.len() {
            let extra = jobs.len() - dispatched.capacity();
            reserve(&mut dispatched, extra, "metal controller dispatch")?;
        }
        let mut touched = Vec::new();
        let mut overwrote_existing = false;
        let mut all_dispatched = true;
        for (index, job) in jobs.iter().enumerate() {
            if self.input_finite.get(index).copied().unwrap_or(0) == 0 {
                all_dispatched = false;
                continue;
            }
            let job_topo = job.controller.topology();
            if (job_topo.input_width(), job_topo.raw_output_width()) != batch_dims {
                all_dispatched = false;
                continue;
            }
            let Ok(topology) = usize::try_from(job.slot.topology()) else {
                all_dispatched = false;
                continue;
            };
            let Some(bucket) = self.bucket(job)? else {
                all_dispatched = false;
                continue;
            };
            let (Some(geometry), Some(shape_words)) = (bucket.geometry, bucket.shape_words) else {
                all_dispatched = false;
                continue;
            };
            let dtype = bucket.dtype;
            let at = geometry.locate(job.slot.slot());
            let local = usize::try_from(at.slot_in_shard).map_err(|_| CoreError::Overflow {
                quantity: "arena slot",
            })?;
            let shard = if let Some(shard) = bucket.shards.get_mut(at.shard) {
                shard
            } else {
                let elements = usize::try_from(geometry.shard_elements()).map_err(|_| {
                    CoreError::Overflow {
                        quantity: "arena shard elements",
                    }
                })?;
                let slots = usize::try_from(geometry.slots_per_shard()).map_err(|_| {
                    CoreError::Overflow {
                        quantity: "arena shard slots",
                    }
                })?;
                let bytes = geometry
                    .shard_elements()
                    .saturating_mul(dtype.bytes())
                    .saturating_add(bytes_for::<Option<(u32, GenomeId)>>(slots))
                    .saturating_add(SHARD_HOST_BYTES);
                let staging = staging_bytes(budget.organisms, shard_count.saturating_add(1));
                if held.saturating_add(bytes).saturating_add(staging) > budget.bytes {
                    all_dispatched = false;
                    continue;
                }
                let made = match dtype {
                    WeightDtype::F32 => owner
                        .zeroed_owned::<f32>(elements)
                        .map(ShardParameters::F32),
                    // F16 slot stride is a multiple of 4 elements, so `elements`
                    // is always even and fills whole `u32` words.
                    WeightDtype::F16 => owner
                        .zeroed_owned::<u32>(elements / 2)
                        .map(ShardParameters::F16),
                };
                let parameters = match made {
                    Ok(parameters) => parameters,
                    Err(CoreError::BudgetExhausted { .. }) => {
                        all_dispatched = false;
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let mut occupants = Vec::new();
                occupants
                    .try_reserve_exact(slots)
                    .map_err(|_| CoreError::AllocationFailed {
                        buffer: "metal controller slots",
                        bytes: bytes_for::<Option<(u32, GenomeId)>>(slots),
                    })?;
                occupants.resize(slots, None);
                held = held.saturating_add(bytes);
                shard_count = shard_count.saturating_add(1);
                bucket.shards.insert(
                    at.shard,
                    DeviceShard {
                        parameters,
                        occupants,
                        live_count: 0,
                        bytes,
                        jobs: None,
                        job_capacity: 0,
                        staged: Vec::new(),
                    },
                )
            };
            let own = Some((job.slot.generation(), job.genome));
            let Some(occupant) = shard.occupants.get_mut(local) else {
                all_dispatched = false;
                continue;
            };
            if *occupant != own {
                let offset = usize::try_from(at.offset).map_err(|_| CoreError::Overflow {
                    quantity: "arena offset",
                })?;
                // The old occupant is gone before the new one is written, so a
                // failure leaves the slot empty, never mixed.
                if occupant.take().is_some() {
                    shard.live_count = shard.live_count.saturating_sub(1);
                    overwrote_existing = true;
                }
                match (&shard.parameters, job.controller.parameters()) {
                    (ShardParameters::F32(buffer), Weights::F32(values)) => {
                        owner.write(buffer.buffer())?.write(offset, values)?;
                    }
                    // Two binary16 values a word, the even one low; F16 slot
                    // stride is a multiple of 4 elements, so `offset` is always
                    // even (and 4-aligned). An odd `values.len()` writes its
                    // trailing element into the low 16 bits of the last word
                    // with zero high-half padding inside this slot's stride.
                    (ShardParameters::F16(words), Weights::F16(values)) => {
                        packed.clear();
                        let word_count = values.len().div_ceil(2);
                        reserve(&mut packed, word_count, "metal binary16 words")?;
                        let (pairs, remainder) = values.as_chunks::<2>();
                        packed.extend(pairs.iter().map(|[low, high]| {
                            u32::from(low.to_bits()) | (u32::from(high.to_bits()) << 16)
                        }));
                        if let Some(&last) = remainder.first() {
                            packed.push(u32::from(last.to_bits()));
                        }
                        owner.write(words.buffer())?.write(offset / 2, &packed)?;
                    }
                    // A bucket serves only its own dtype (`bucket`).
                    _ => {
                        all_dispatched = false;
                        continue;
                    }
                }
                *occupant = own;
                shard.live_count += 1;
                uploads += 1;
            }
            let row = dispatched.len();
            dispatched.push(index);
            if !overwrote_existing {
                let row_word = u32::try_from(row).map_err(|_| CoreError::LayoutMismatch {
                    buffer: "metal controller batch",
                    detail: format!("row {row} exceeds u32"),
                })?;
                let offset_word =
                    u32::try_from(at.offset).map_err(|_| CoreError::LayoutMismatch {
                        buffer: "metal controller batch",
                        detail: format!("parameter offset {} exceeds u32", at.offset),
                    })?;
                if shard.staged.is_empty() {
                    reserve(&mut touched, 1, "metal controller shards")?;
                    touched.push((topology, at.shard));
                }
                reserve(&mut shard.staged, JOB_WORDS, "metal jobs")?;
                shard.staged.extend_from_slice(&[
                    row_word,
                    offset_word,
                    shape_words[0],
                    shape_words[1],
                    shape_words[2],
                ]);
            }
        }
        self.uploads += uploads;
        (self.held, self.shard_count) = (held, shard_count);
        self.dispatched = dispatched;

        if overwrote_existing {
            // At least one live slot was overwritten during this batch; drop
            // any earlier job whose slot was evicted by a later job in the
            // same batch and rebuild per-shard job lists cleanly.
            touched.clear();
            for bucket in self.buckets.iter_mut().flatten() {
                for shard in bucket.shards.values_mut() {
                    shard.staged.clear();
                }
            }
            let candidates = std::mem::take(&mut self.dispatched);
            for &index in &candidates {
                let Some(job) = jobs.get(index) else { continue };
                let Ok(topology) = usize::try_from(job.slot.topology()) else {
                    all_dispatched = false;
                    continue;
                };
                let Some(bucket) = self.buckets.get_mut(topology).and_then(Option::as_mut) else {
                    all_dispatched = false;
                    continue;
                };
                let (Some(geometry), Some(shape_words)) = (bucket.geometry, bucket.shape_words)
                else {
                    all_dispatched = false;
                    continue;
                };
                let at = geometry.locate(job.slot.slot());
                let Some(shard) = bucket.shards.get_mut(at.shard) else {
                    all_dispatched = false;
                    continue;
                };
                let resident = usize::try_from(at.slot_in_shard)
                    .ok()
                    .and_then(|local| shard.occupants.get(local).copied())
                    .flatten()
                    == Some((job.slot.generation(), job.genome));
                if !resident {
                    all_dispatched = false;
                    continue;
                }
                let row = self.dispatched.len();
                self.dispatched.push(index);
                let row_word = u32::try_from(row).map_err(|_| CoreError::LayoutMismatch {
                    buffer: "metal controller batch",
                    detail: format!("row {row} exceeds u32"),
                })?;
                let offset_word =
                    u32::try_from(at.offset).map_err(|_| CoreError::LayoutMismatch {
                        buffer: "metal controller batch",
                        detail: format!("parameter offset {} exceeds u32", at.offset),
                    })?;
                if shard.staged.is_empty() {
                    reserve(&mut touched, 1, "metal controller shards")?;
                    touched.push((topology, at.shard));
                }
                reserve(&mut shard.staged, JOB_WORDS, "metal jobs")?;
                shard.staged.extend_from_slice(&[
                    row_word,
                    offset_word,
                    shape_words[0],
                    shape_words[1],
                    shape_words[2],
                ]);
            }
        }
        touched.sort_unstable();
        Ok((touched, all_dispatched))
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
        let organisms = count.next_power_of_two().max(256);
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
/// single pass (parallelized for large batches).
fn narrow_inputs(
    jobs: &[ControllerJob<'_>],
    input_width: usize,
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
    if jobs.len() < PARALLEL_INPUT_MIN_JOBS {
        narrow_chunk(jobs, out, flags);
    } else {
        let threads = std::thread::available_parallelism()
            .map_or(4, usize::from)
            .clamp(1, PARALLEL_INPUT_MAX_THREADS)
            .min(jobs.len().div_ceil(1024));
        let chunk = jobs.len().div_ceil(threads);
        std::thread::scope(|scope| {
            let mut job_chunks = jobs.chunks(chunk);
            let mut out_chunks = out.chunks_mut(chunk * input_width);
            let mut flag_chunks = flags.chunks_mut(chunk);
            let head = (job_chunks.next(), out_chunks.next(), flag_chunks.next());
            let mut handles = Vec::with_capacity(threads.saturating_sub(1));
            for ((jc, oc), fc) in job_chunks.zip(out_chunks).zip(flag_chunks) {
                handles.push(scope.spawn(move || narrow_chunk(jc, oc, fc)));
            }
            if let (Some(jc), Some(oc), Some(fc)) = head {
                narrow_chunk(jc, oc, fc);
            }
            for handle in handles {
                let _ = handle.join();
            }
        });
    }
    Ok(())
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
            capacity_bytes(&self.values),
            capacity_bytes(&self.flags),
        ]))
    }

    fn forget(&mut self) {
        self.buckets.clear();
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

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<RawOutputs>>,
    ) -> Result<(), CoreError> {
        outputs.clear();
        reserve(outputs, jobs.len(), "metal controller outputs")?;
        self.last_command = None;
        if jobs.is_empty() {
            return Ok(());
        }
        let Some((input_width, output_width)) = jobs.iter().find_map(|job| {
            let topo = job.controller.topology();
            let len = job.controller.parameters().len();
            (job.inputs.len() == topo.input_width() && job_words(topo, len, 0, 0).is_ok())
                .then_some((topo.input_width(), topo.raw_output_width()))
        }) else {
            outputs.resize(jobs.len(), None);
            return Ok(());
        };
        narrow_inputs(
            jobs,
            input_width,
            &mut self.staged_inputs,
            &mut self.input_finite,
        )?;
        let (touched, all_dispatched) = self.place_and_stage(jobs, (input_width, output_width))?;
        let count = self.dispatched.len();
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
            self.hold(count, input_width, output_width)?;
            let owner = self.controllers.owner.clone();
            for &(topology, key) in &touched {
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
                    let capacity = needed.next_power_of_two().max(64);
                    shard.jobs = None;
                    shard.jobs = Some(owner.zeroed_owned(capacity * JOB_WORDS)?);
                    shard.job_capacity = capacity;
                }
                if let Some(words) = shard.jobs.as_ref() {
                    owner.write(words.buffer())?.write(0, &shard.staged)?;
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
            owner.write(dims)?.write(0, &dims_words)?;
            owner
                .write(inputs)?
                .write(0, &self.staged_inputs[..count * input_width])?;
            let mut bindings = Vec::new();
            reserve(&mut bindings, touched.len(), "metal controller kernels")?;
            for &(topology, key) in &touched {
                let bucket = self.buckets.get(topology).and_then(Option::as_ref);
                let shard = bucket.and_then(|bucket| bucket.shards.get(key));
                let Some((dtype, parameters, words, staged)) =
                    bucket.zip(shard).and_then(|(bucket, shard)| {
                        Some((
                            bucket.dtype,
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
                ));
            }
            let kernels: Vec<Kernel<'_>> = bindings
                .iter()
                .map(|(pipeline, bound, staged)| Kernel {
                    pipeline,
                    bindings: bound,
                    grid: Grid::linear(*staged, FORWARD_THREADS),
                })
                .collect();
            let receipt = owner.submit_batch(&kernels)?;
            self.last_command = Some((receipt.kernels, receipt.device_seconds));
            let need_values = count * output_width;
            if self.values.len() < need_values {
                let extra = need_values - self.values.len();
                reserve(&mut self.values, extra, "metal controller outputs")?;
                self.values.resize(need_values, 0.0);
            }
            owner.read_back(raw, &mut self.values[..need_values])?;
            if self.flags.len() < count {
                let extra = count - self.flags.len();
                reserve(&mut self.flags, extra, "metal controller faults")?;
                self.flags.resize(count, 0);
            }
            owner.read_back(faults, &mut self.flags[..count])?;
        }
        if count == jobs.len() {
            for (row, &flag) in self.values[..count * output_width]
                .chunks_exact(output_width)
                .zip(&self.flags[..count])
            {
                outputs.push(if flag == 0 {
                    Some(RawOutputs::from_slice(row)?)
                } else {
                    None
                });
            }
        } else {
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
                            Some(RawOutputs::from_slice(row)?)
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
/// Measured on the preset (M5 Ultra, `flife run`, whole ticks): Metal is
/// 2.2x faster on one worker, 1.7x on two, 1.3x on four and 1.1x on eight,
/// and no faster on sixteen, where the serial phases bound the tick. In the
/// Studio on sixteen, whose presentation uses the same GPU, it measured
/// slower under FrankenManim dfdc7d18 (72 against 119 ticks per second, one
/// 12 s run each) and level under f401a947 (132 against 120 at 960x540, 152
/// against 161 at 1920x1080, four interleaved 10 s runs each).
pub const AUTO_METAL_MAX_WORKERS: usize = 8;

/// The most CPU workers at which [`ControllerChoice::Auto`] takes
/// FrankenTorch's CPU route where it does not take Metal (FL-025). Measured
/// on the preset (`tick_throughput`, whole ticks, each against the reference
/// in the same process, cv at most 5%, the same digests): 1.41x on one
/// worker, 1.11x on four and 0.91x on eight on Linux x86-64 (2026-11-10),
/// and 1.62x on one and 1.26x on four on an M5 Ultra.
pub const AUTO_TORCH_MAX_WORKERS: usize = 4;

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
        let metal = || MetalBackend::with_budget(u64::MAX, budget);
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
