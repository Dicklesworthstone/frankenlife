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
    ACTION_WIDTH, ControllerBackend, ControllerJob, DenseTopology, SENSOR_WIDTH, ShardGeometry,
    WeightDtype, Weights,
};
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

pub mod chemistry;
pub mod field;
pub mod motion;
pub mod owner;
pub mod sensors;
pub mod uptake;

pub use chemistry::{
    ChemistryConstants, ChemistryOrganism, ChemistryOutcome, ChemistryPacket, MetalChemistry,
    SETTLE_OPS_WORDS,
};
pub use field::MetalField;
/// The memory a controller backend may hold (FL-043), for composition
/// roots that make one through [`ControllerChoice::backend`].
pub use flife_neural::BackendBudget;
pub use motion::{
    ExhaustedAt, MOTION_OPS_WORDS, MOTION_TERMS, MaintenanceOutcome, MetalMotion, MotionConstants,
    MotionOrganism, MotionOutcome, MovementOutcome,
};
use owner::metal;
pub use owner::{
    Binding, Buffer, DeviceOwner, Element, Kernel, Limits, Owned, Purpose, ReadLease, Receipt,
    ResourceState, Usage, WriteLease,
};
pub use sensors::MetalSensors;
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
/// reference rounds them, and its topology.
#[derive(Debug, Default, Clone)]
pub struct Batch {
    parameters: Vec<f32>,
    inputs: Vec<f32>,
    jobs: Vec<u32>,
}

impl Batch {
    /// An empty batch.
    #[must_use]
    pub const fn new() -> Self {
        Self {
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
        self.parameters.clear();
        self.inputs.clear();
        self.jobs.clear();
    }

    /// Add one organism: its controller's topology and parameters, and its
    /// observation's inputs.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for an input that is not finite in
    /// F32, exactly as the reference refuses it;
    /// [`CoreError::LayoutMismatch`] for parameters that do not fill the
    /// topology, or a topology outside the kernel's shape (one or two
    /// hidden layers, each at most [`WIDEST_HIDDEN`] wide);
    /// [`CoreError::AllocationFailed`] if the batch cannot grow. The batch is
    /// unchanged on error.
    pub fn push(
        &mut self,
        topology: &DenseTopology,
        parameters: &[f32],
        inputs: &[f64; SENSOR_WIDTH],
    ) -> Result<(), CoreError> {
        let mut rounded = [0.0_f32; SENSOR_WIDTH];
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
        let job = job_words(
            topology,
            parameters.len(),
            self.parameters.len(),
            self.len(),
        )?;
        reserve(&mut self.parameters, parameters.len(), "batch parameters")?;
        reserve(&mut self.inputs, SENSOR_WIDTH, "batch inputs")?;
        reserve(&mut self.jobs, JOB_WORDS, "batch jobs")?;
        self.parameters.extend_from_slice(parameters);
        self.inputs.extend_from_slice(&rounded);
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
    let mut fan_in = SENSOR_WIDTH;
    for (index, layer) in topology.layers().iter().enumerate() {
        let fan_out = widths
            .get(index)
            .map_or(ACTION_WIDTH, |&width| width as usize);
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

/// A batch on the device: its parameters, inputs and jobs, and room for its
/// outputs. Its buffers are retired when it is dropped.
pub struct Resident {
    organisms: usize,
    parameters: Owned<f32>,
    inputs: Owned<f32>,
    jobs: Owned<u32>,
    raw: Owned<f32>,
    faults: Owned<u32>,
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
        if organisms == 0 {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal controller batch",
                detail: "an empty batch".to_owned(),
            });
        }
        Ok(Resident {
            organisms,
            parameters: self.owner.upload_owned(&batch.parameters)?,
            inputs: self.owner.upload_owned(&batch.inputs)?,
            jobs: self.owner.upload_owned(&batch.jobs)?,
            raw: self.owner.zeroed_owned(organisms * ACTION_WIDTH)?,
            faults: self.owner.zeroed_owned(organisms)?,
        })
    }

    /// Evaluate every organism of an uploaded batch, in its order. Each
    /// entry is the organism's 36 raw outputs, bit-identical to the
    /// reference controller's, or `None` for a deferred organism (see the
    /// crate documentation): evaluate that one on the reference controller.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if the dispatch fails;
    /// [`CoreError::AllocationFailed`] if `outputs` cannot grow.
    pub fn run(
        &self,
        resident: &Resident,
        outputs: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
    ) -> Result<(), CoreError> {
        outputs.clear();
        let organisms = resident.organisms;
        reserve(outputs, organisms, "metal controller outputs")?;
        self.owner.submit(
            &self.forward,
            &[
                resident.parameters.buffer().reads(),
                resident.inputs.buffer().reads(),
                resident.jobs.buffer().reads(),
                resident.raw.buffer().writes(),
                resident.faults.buffer().writes(),
            ],
            Grid::linear(organisms, FORWARD_THREADS),
        )?;
        let mut values = vec![0.0_f32; organisms * ACTION_WIDTH];
        self.owner.read_back(resident.raw.buffer(), &mut values)?;
        let mut flags = vec![0_u32; organisms];
        self.owner.read_back(resident.faults.buffer(), &mut flags)?;
        for (row, &flag) in values.as_chunks::<ACTION_WIDTH>().0.iter().zip(&flags) {
            outputs.push((flag == 0).then_some(*row));
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
        outputs: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
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
/// buffers, a power of two of at least 256 rows; each shard's job words, a
/// power of two of at least 64 jobs; and the host's staged inputs, outputs,
/// fault flags, dispatch lists and job words, which grow by doubling.
#[must_use]
pub const fn staging_bytes(organisms: u64, shards: u64) -> u64 {
    let rows = match organisms.checked_next_power_of_two() {
        Some(rows) if rows < 256 => 256,
        Some(rows) => rows,
        None => u64::MAX,
    };
    let row = ((SENSOR_WIDTH + ACTION_WIDTH + 1) * size_of::<f32>()) as u64;
    let words = (JOB_WORDS * size_of::<u32>()) as u64;
    let device = rows.saturating_mul(row).saturating_add(
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
    /// kernel reads the buffer as `half`. A slot holds an even number of
    /// elements, so no word is shared by two slots.
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
/// made when its first slot is needed and never moved or rebuilt.
struct DeviceShard {
    parameters: ShardParameters,
    /// Each slot's uploaded occupant, its generation and genome, or `None`
    /// before its first.
    occupants: Vec<Option<(u32, GenomeId)>>,
    /// This shard's job words for the batch, and their capacity in jobs.
    jobs: Option<Owned<u32>>,
    job_capacity: usize,
    staged: Vec<u32>,
}

/// One catalog member, as this backend first saw it.
struct DeviceBucket {
    topology: DenseTopology,
    /// The weight dtype of its first genome, which its shards hold; a job of
    /// another dtype is deferred.
    dtype: WeightDtype,
    /// `None` when not one slot fits a device shard: its jobs are deferred.
    geometry: Option<ShardGeometry>,
    shards: HashMap<u64, DeviceShard>,
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
    /// Shards made, and the bytes they hold, device and host.
    shard_count: u64,
    held: u64,
    buckets: Vec<Option<DeviceBucket>>,
    /// Per-organism buffers, sized for `organisms`.
    organisms: usize,
    inputs: Option<Owned<f32>>,
    raw: Option<Owned<f32>>,
    faults: Option<Owned<u32>>,
    /// Host staging, reused.
    staged_inputs: Vec<f32>,
    eligible: Vec<usize>,
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
    /// # Errors
    /// As [`MetalControllers::open`].
    pub fn with_budget(shard_slots: u64, budget: BackendBudget) -> Result<Self, CoreError> {
        Ok(Self {
            controllers: MetalControllers::open_with(Limits {
                bytes: budget.bytes,
                leases_per_purpose: u32::MAX,
            })?,
            shard_slots: shard_slots.max(1),
            budget,
            shard_count: 0,
            held: 0,
            buckets: Vec::new(),
            organisms: 0,
            inputs: None,
            raw: None,
            faults: None,
            staged_inputs: Vec::new(),
            eligible: Vec::new(),
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
            // Binary16 slots start on a word: an odd parameter count would
            // share one with the next slot, so such a member is deferred.
            let packs = dtype == WeightDtype::F32 || length.is_multiple_of(2);
            DeviceBucket {
                topology: topology.clone(),
                dtype,
                geometry: packs
                    .then(|| {
                        ShardGeometry::fitting(length, limit.min(u64::from(u32::MAX)), shard_slots)
                            .ok()
                    })
                    .flatten(),
                shards: HashMap::new(),
            }
        });
        let serves = bucket.topology == *topology
            && bucket.dtype == dtype
            && bucket
                .geometry
                .is_some_and(|geometry| geometry.parameters() == length);
        Ok(serves.then_some(bucket))
    }

    /// Copy each eligible job's parameters to its slot unless they are
    /// resident, then keep for dispatch the jobs whose slots hold their own
    /// parameters: a slot two jobs of one batch name holds the last one's.
    fn place(&mut self, jobs: &[ControllerJob<'_>]) -> Result<(), CoreError> {
        let eligible = std::mem::take(&mut self.eligible);
        let owner = self.controllers.owner.clone();
        let mut uploads = 0;
        let budget = self.budget;
        // What the shards hold, written back once the batch is placed. An
        // error drops the backend, so a lost count is never used.
        let (mut held, mut shard_count) = (self.held, self.shard_count);
        let mut packed: Vec<u32> = Vec::new();
        for &index in &eligible {
            let Some(job) = jobs.get(index) else { continue };
            let Some(bucket) = self.bucket(job)? else {
                continue;
            };
            let Some(geometry) = bucket.geometry else {
                continue;
            };
            let dtype = bucket.dtype;
            let at = geometry.locate(job.slot.slot());
            let local = usize::try_from(at.slot_in_shard).map_err(|_| CoreError::Overflow {
                quantity: "arena slot",
            })?;
            let shard = match bucket.shards.entry(at.shard) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
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
                        continue;
                    }
                    let made = match dtype {
                        WeightDtype::F32 => owner
                            .zeroed_owned::<f32>(elements)
                            .map(ShardParameters::F32),
                        // Slots of an even count fill whole words.
                        WeightDtype::F16 => owner
                            .zeroed_owned::<u32>(elements / 2)
                            .map(ShardParameters::F16),
                    };
                    let parameters = match made {
                        Ok(parameters) => parameters,
                        Err(CoreError::BudgetExhausted { .. }) => continue,
                        Err(error) => return Err(error),
                    };
                    let mut occupants = Vec::new();
                    occupants.try_reserve_exact(slots).map_err(|_| {
                        CoreError::AllocationFailed {
                            buffer: "metal controller slots",
                            bytes: bytes_for::<Option<(u32, GenomeId)>>(slots),
                        }
                    })?;
                    occupants.resize(slots, None);
                    held = held.saturating_add(bytes);
                    shard_count = shard_count.saturating_add(1);
                    entry.insert(DeviceShard {
                        parameters,
                        occupants,
                        jobs: None,
                        job_capacity: 0,
                        staged: Vec::new(),
                    })
                }
            };
            let own = Some((job.slot.generation(), job.genome));
            let Some(occupant) = shard.occupants.get_mut(local) else {
                continue;
            };
            if *occupant == own {
                continue;
            }
            let offset = usize::try_from(at.offset).map_err(|_| CoreError::Overflow {
                quantity: "arena offset",
            })?;
            // The old occupant is gone before the new one is written, so a
            // failure leaves the slot empty, never mixed.
            *occupant = None;
            match (&shard.parameters, job.controller.parameters()) {
                (ShardParameters::F32(buffer), Weights::F32(values)) => {
                    owner.write(buffer.buffer())?.write(offset, values)?;
                }
                // Two binary16 values a word, the even one low; the slot's
                // first word is its even offset halved.
                (ShardParameters::F16(words), Weights::F16(values)) => {
                    packed.clear();
                    reserve(&mut packed, values.len() / 2, "metal binary16 words")?;
                    packed.extend(values.as_chunks::<2>().0.iter().map(|[low, high]| {
                        u32::from(low.to_bits()) | (u32::from(high.to_bits()) << 16)
                    }));
                    owner.write(words.buffer())?.write(offset / 2, &packed)?;
                }
                // A bucket serves only its own dtype (`bucket`).
                _ => continue,
            }
            *occupant = own;
            uploads += 1;
        }
        self.uploads += uploads;
        (self.held, self.shard_count) = (held, shard_count);
        self.dispatched.clear();
        for &index in &eligible {
            let Some(job) = jobs.get(index) else { continue };
            let Some(bucket) = self.bucket(job)? else {
                continue;
            };
            let resident = bucket.geometry.is_some_and(|geometry| {
                let at = geometry.locate(job.slot.slot());
                bucket
                    .shards
                    .get(&at.shard)
                    .and_then(|shard| {
                        shard
                            .occupants
                            .get(usize::try_from(at.slot_in_shard).ok()?)
                            .copied()
                    })
                    .flatten()
                    == Some((job.slot.generation(), job.genome))
            });
            if resident {
                reserve(&mut self.dispatched, 1, "metal controller dispatch")?;
                self.dispatched.push(index);
            }
        }
        self.eligible = eligible;
        Ok(())
    }

    /// Grow the per-organism buffers to hold `count` organisms.
    fn hold(&mut self, count: usize) -> Result<(), CoreError> {
        if count <= self.organisms && self.inputs.is_some() {
            return Ok(());
        }
        let organisms = count.next_power_of_two().max(256);
        // Retire the old buffers first, so their bytes are free again.
        (self.inputs, self.raw, self.faults) = (None, None, None);
        let owner = &self.controllers.owner;
        self.inputs = Some(owner.zeroed_owned(organisms * SENSOR_WIDTH)?);
        self.raw = Some(owner.zeroed_owned(organisms * ACTION_WIDTH)?);
        self.faults = Some(owner.zeroed_owned(organisms)?);
        self.organisms = organisms;
        Ok(())
    }

    /// Stage every dispatched job: its inputs at its row, and its job words
    /// in its shard's list. Returns each touched shard's key, in order.
    fn stage(&mut self, jobs: &[ControllerJob<'_>]) -> Result<Vec<(usize, u64)>, CoreError> {
        let count = self.dispatched.len();
        self.staged_inputs.clear();
        reserve(
            &mut self.staged_inputs,
            count * SENSOR_WIDTH,
            "metal inputs",
        )?;
        for bucket in self.buckets.iter_mut().flatten() {
            for shard in bucket.shards.values_mut() {
                shard.staged.clear();
            }
        }
        let mut touched = Vec::new();
        for (row, &index) in self.dispatched.iter().enumerate() {
            let Some(job) = jobs.get(index) else { continue };
            let topology = usize::try_from(job.slot.topology()).unwrap_or(usize::MAX);
            let Some(bucket) = self.buckets.get_mut(topology).and_then(Option::as_mut) else {
                continue;
            };
            let Some(geometry) = bucket.geometry else {
                continue;
            };
            let at = geometry.locate(job.slot.slot());
            let Some(shard) = bucket.shards.get_mut(&at.shard) else {
                continue;
            };
            let offset = usize::try_from(at.offset).unwrap_or(usize::MAX);
            let words = job_words(
                job.controller.topology(),
                job.controller.parameters().len(),
                offset,
                row,
            )?;
            if shard.staged.is_empty() {
                reserve(&mut touched, 1, "metal controller shards")?;
                touched.push((topology, at.shard));
            }
            reserve(&mut shard.staged, JOB_WORDS, "metal jobs")?;
            shard.staged.extend_from_slice(&words);
            self.staged_inputs
                .extend(job.inputs.iter().map(|&value| value as f32));
        }
        touched.sort_unstable();
        Ok(touched)
    }
}

impl ControllerBackend for MetalBackend {
    fn describe(&self) -> String {
        format!("metal ({})", self.controllers.device_name())
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
            capacity_bytes(&self.eligible),
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

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
    ) -> Result<(), CoreError> {
        outputs.clear();
        reserve(outputs, jobs.len(), "metal controller outputs")?;
        self.last_command = None;
        self.eligible.clear();
        // A job the kernel cannot take, or whose inputs the reference
        // refuses, is deferred: the reference evaluates it, or refuses it.
        for (index, entry) in jobs.iter().enumerate() {
            let finite = entry.inputs.iter().all(|&value| (value as f32).is_finite());
            let shaped = job_words(
                entry.controller.topology(),
                entry.controller.parameters().len(),
                0,
                0,
            )
            .is_ok();
            if finite && shaped {
                reserve(&mut self.eligible, 1, "metal controller dispatch")?;
                self.eligible.push(index);
            }
        }
        self.dispatched.clear();
        if !self.eligible.is_empty() {
            self.place(jobs)?;
        }
        let count = self.dispatched.len();
        if count > 0 {
            self.hold(count)?;
            let touched = self.stage(jobs)?;
            let owner = self.controllers.owner.clone();
            for &(topology, key) in &touched {
                let Some(shard) = self
                    .buckets
                    .get_mut(topology)
                    .and_then(Option::as_mut)
                    .and_then(|bucket| bucket.shards.get_mut(&key))
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
            let (Some(inputs), Some(raw), Some(faults)) = (
                self.inputs.as_ref().map(Owned::buffer),
                self.raw.as_ref().map(Owned::buffer),
                self.faults.as_ref().map(Owned::buffer),
            ) else {
                return Err(CoreError::StateConflict {
                    operation: "metal controller residency",
                    detail: "buffers missing".to_owned(),
                });
            };
            owner.write(inputs)?.write(0, &self.staged_inputs)?;
            let mut bindings = Vec::new();
            reserve(&mut bindings, touched.len(), "metal controller kernels")?;
            for &(topology, key) in &touched {
                let bucket = self.buckets.get(topology).and_then(Option::as_ref);
                let shard = bucket.and_then(|bucket| bucket.shards.get(&key));
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
            self.values.clear();
            self.values.resize(count * ACTION_WIDTH, 0.0);
            owner.read_back(raw, &mut self.values)?;
            self.flags.clear();
            self.flags.resize(count, 0);
            owner.read_back(faults, &mut self.flags)?;
        }
        let mut dispatched = self.dispatched.iter().zip(
            self.values
                .as_chunks::<ACTION_WIDTH>()
                .0
                .iter()
                .zip(&self.flags),
        );
        let mut next = dispatched.next();
        for index in 0..jobs.len() {
            match next {
                Some((&at, (row, &flag))) if at == index => {
                    outputs.push((flag == 0).then_some(*row));
                    next = dispatched.next();
                }
                _ => outputs.push(None),
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
    /// [`ControllerChoice::Torch`] on at most [`AUTO_TORCH_MAX_WORKERS`];
    /// otherwise the CPU reference.
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

impl ControllerChoice {
    /// What [`ControllerChoice::Auto`] is for a world on `cpu_workers`
    /// threads on a host with or without a Metal device: Metal up to
    /// [`AUTO_METAL_MAX_WORKERS`] where there is one, otherwise `torch` up to
    /// [`AUTO_TORCH_MAX_WORKERS`], otherwise the CPU reference.
    #[must_use]
    pub const fn auto_for(cpu_workers: usize, metal_device: bool) -> Self {
        if metal_device && cpu_workers <= AUTO_METAL_MAX_WORKERS {
            Self::Metal
        } else if cpu_workers <= AUTO_TORCH_MAX_WORKERS {
            Self::Torch
        } else {
            Self::Cpu
        }
    }

    /// The backend this choice selects on this host for a world on
    /// `cpu_workers` threads, or `None` for the CPU reference.
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
                if Self::auto_for(cpu_workers, true) == Self::Metal {
                    match metal() {
                        Ok(backend) => return Ok(Some(boxed(backend))),
                        Err(CoreError::MissingCapability { .. }) => {}
                        Err(error) => return Err(error),
                    }
                }
                Self::auto_for(cpu_workers, false).backend(cpu_workers, budget)
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
