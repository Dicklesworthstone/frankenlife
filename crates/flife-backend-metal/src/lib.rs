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

use flife_core::CoreError;
use flife_core::ids::GenomeId;
use flife_neural::{ACTION_WIDTH, ControllerBackend, ControllerJob, DenseTopology, SENSOR_WIDTH};
use ft_kernel_metal::Error as MetalError;
use ft_kernel_metal::compute::{Gateway, Grid, Pipeline, SharedBuffer};

/// The kernel source.
pub const CONTROLLER_MSL: &str = include_str!("controller.metal");

/// The widest hidden layer the kernel's threadgroup storage holds.
pub const WIDEST_HIDDEN: usize = 128;

/// Threads per organism in the forward dispatch.
pub const FORWARD_THREADS: usize = 64;

/// The jobs words per organism: parameter offset, hidden-layer count and
/// two hidden widths.
const JOB_WORDS: usize = 4;

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
        let job = job(topology, parameters.len(), self.parameters.len())?;
        reserve(&mut self.parameters, parameters.len(), "batch parameters")?;
        reserve(&mut self.inputs, SENSOR_WIDTH, "batch inputs")?;
        reserve(&mut self.jobs, JOB_WORDS, "batch jobs")?;
        self.parameters.extend_from_slice(parameters);
        self.inputs.extend_from_slice(&rounded);
        self.jobs.extend_from_slice(&job);
        Ok(())
    }
}

/// The kernel's job words for `topology` at `offset`, after checking that
/// its layers sit back to back from zero as the kernel walks them.
fn job(topology: &DenseTopology, length: usize, offset: usize) -> Result<[u32; 4], CoreError> {
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
    Ok([
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
/// outputs.
pub struct Resident {
    organisms: usize,
    parameters: SharedBuffer,
    inputs: SharedBuffer,
    jobs: SharedBuffer,
    raw: SharedBuffer,
    faults: SharedBuffer,
}

impl std::fmt::Debug for Resident {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Resident")
            .field("organisms", &self.organisms)
            .field("parameter_bytes", &self.parameters.len_bytes())
            .finish_non_exhaustive()
    }
}

/// The compiled controller kernels on this host's Metal device.
pub struct MetalControllers {
    gateway: Gateway,
    forward: Pipeline,
    activation: Pipeline,
    binary64: Pipeline,
    binary32: Pipeline,
}

impl std::fmt::Debug for MetalControllers {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalControllers")
            .field("device", &self.gateway.device_name())
            .finish_non_exhaustive()
    }
}

fn metal(error: MetalError) -> CoreError {
    match error {
        MetalError::Unavailable => CoreError::MissingCapability {
            capability: "a Metal device".to_owned(),
        },
        other => CoreError::StateConflict {
            operation: "metal controller dispatch",
            detail: other.to_string(),
        },
    }
}

impl MetalControllers {
    /// Open the default Metal device and compile the kernels.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] without a Metal device (always, off
    /// macOS); [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn open() -> Result<Self, CoreError> {
        let gateway = Gateway::open().map_err(metal)?;
        let library = gateway.library(CONTROLLER_MSL).map_err(metal)?;
        let pipeline = |name: &str| library.pipeline(name).map_err(metal);
        Ok(Self {
            forward: pipeline("controller_forward")?,
            activation: pipeline("activation_bits")?,
            binary64: pipeline("binary64_ops")?,
            binary32: pipeline("binary32_ops")?,
            gateway,
        })
    }

    /// The device's name.
    #[must_use]
    pub fn device_name(&self) -> String {
        self.gateway.device_name()
    }

    /// Copy `batch` to the device, for [`Self::run`].
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for an empty batch;
    /// [`CoreError::StateConflict`] if the device refuses a buffer.
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
            parameters: self.gateway.buffer_f32(&batch.parameters).map_err(metal)?,
            inputs: self.gateway.buffer_f32(&batch.inputs).map_err(metal)?,
            jobs: self.gateway.buffer_u32(&batch.jobs).map_err(metal)?,
            raw: self
                .gateway
                .buffer_zeroed(organisms * ACTION_WIDTH * size_of::<f32>())
                .map_err(metal)?,
            faults: self
                .gateway
                .buffer_zeroed(organisms * size_of::<u32>())
                .map_err(metal)?,
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
        self.gateway
            .dispatch(
                &self.forward,
                &[
                    &resident.parameters,
                    &resident.inputs,
                    &resident.jobs,
                    &resident.raw,
                    &resident.faults,
                ],
                Grid::linear(organisms, FORWARD_THREADS),
            )
            .map_err(metal)?;
        let mut values = vec![0.0_f32; organisms * ACTION_WIDTH];
        resident.raw.read_f32(&mut values).map_err(metal)?;
        let mut flags = vec![0_u32; organisms];
        resident.faults.read_u32(&mut flags).map_err(metal)?;
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
        let start = self
            .gateway
            .buffer_u32(&[base, u32::from(software_only)])
            .map_err(metal)?;
        let results = self
            .gateway
            .buffer_zeroed(count * size_of::<u32>())
            .map_err(metal)?;
        let fallbacks = self
            .gateway
            .buffer_zeroed(size_of::<u32>())
            .map_err(metal)?;
        self.gateway
            .dispatch(
                &self.activation,
                &[&start, &results, &fallbacks],
                Grid::linear(count / 256, 256),
            )
            .map_err(metal)?;
        results.read_u32(out).map_err(metal)?;
        let mut fell = [0_u32];
        fallbacks.read_u32(&mut fell).map_err(metal)?;
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
        let operands = self.gateway.buffer_u32(&words).map_err(metal)?;
        let results = self
            .gateway
            .buffer_zeroed(pairs.len() * BINARY64_RESULT_WORDS * size_of::<u32>())
            .map_err(metal)?;
        self.gateway
            .dispatch(
                &self.binary64,
                &[&operands, &results],
                Grid::linear(pairs.len() / 64, 64),
            )
            .map_err(metal)?;
        let mut flat = vec![0_u32; pairs.len() * BINARY64_RESULT_WORDS];
        results.read_u32(&mut flat).map_err(metal)?;
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
        let operands = self.gateway.buffer_f32(&flat).map_err(metal)?;
        let results = self
            .gateway
            .buffer_zeroed(pairs.len() * 3 * size_of::<f32>())
            .map_err(metal)?;
        self.gateway
            .dispatch(
                &self.binary32,
                &[&operands, &results],
                Grid::linear(pairs.len() / 64, 64),
            )
            .map_err(metal)?;
        let mut values = vec![0.0_f32; pairs.len() * 3];
        results.read_f32(&mut values).map_err(metal)?;
        out.clear();
        out.extend(values.as_chunks::<3>().0.iter().copied());
        Ok(())
    }
}

/// Where one genome's parameters sit in the resident buffer, and which
/// memory they were copied from.
#[derive(Debug, Clone, Copy)]
struct Slot {
    offset: usize,
    address: usize,
    length: usize,
}

/// [`MetalControllers`] behind the engine's controller port, with every
/// buffer resident. A genome's parameters are copied to the device once and
/// reused while it lives; each tick writes only the inputs and job words in
/// place. When the parameter buffer fills, it is rebuilt from the current
/// batch alone, which drops the genomes that died.
pub struct MetalBackend {
    controllers: MetalControllers,
    slots: HashMap<GenomeId, Slot>,
    parameters: Option<SharedBuffer>,
    /// F32s the parameter buffer holds, and how many are in use.
    capacity: usize,
    used: usize,
    /// Per-organism buffers, sized for `organisms`.
    organisms: usize,
    inputs: Option<SharedBuffer>,
    jobs: Option<SharedBuffer>,
    raw: Option<SharedBuffer>,
    faults: Option<SharedBuffer>,
    /// Host staging, reused.
    staged_inputs: Vec<f32>,
    staged_jobs: Vec<u32>,
    dispatched: Vec<usize>,
    values: Vec<f32>,
    flags: Vec<u32>,
}

impl std::fmt::Debug for MetalBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalBackend")
            .field("device", &self.controllers.device_name())
            .field("resident_genomes", &self.slots.len())
            .field("parameter_capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl MetalBackend {
    /// Open the default Metal device and compile the kernels.
    ///
    /// # Errors
    /// As [`MetalControllers::open`].
    pub fn open() -> Result<Self, CoreError> {
        Ok(Self {
            controllers: MetalControllers::open()?,
            slots: HashMap::new(),
            parameters: None,
            capacity: 0,
            used: 0,
            organisms: 0,
            inputs: None,
            jobs: None,
            raw: None,
            faults: None,
            staged_inputs: Vec::new(),
            staged_jobs: Vec::new(),
            dispatched: Vec::new(),
            values: Vec::new(),
            flags: Vec::new(),
        })
    }

    /// Genomes whose parameters are resident.
    #[must_use]
    pub fn resident_genomes(&self) -> usize {
        self.slots.len()
    }

    /// Make every dispatched job's parameters resident, rebuilding the
    /// buffer from this batch alone when they do not fit.
    fn place(&mut self, jobs: &[ControllerJob<'_>]) -> Result<(), CoreError> {
        let mut missing = 0_usize;
        for &index in &self.dispatched {
            let Some(job) = jobs.get(index) else { continue };
            let parameters = job.controller.parameters();
            let hit = self.slots.get(&job.genome).is_some_and(|slot| {
                slot.address == parameters.as_ptr() as usize && slot.length == parameters.len()
            });
            if !hit {
                missing = missing.saturating_add(parameters.len());
            }
        }
        if self.parameters.is_none() || self.used.saturating_add(missing) > self.capacity {
            let needed: usize = self
                .dispatched
                .iter()
                .filter_map(|&index| jobs.get(index))
                .map(|job| job.controller.parameters().len())
                .sum();
            let capacity = needed.saturating_add(needed / 2).max(1 << 16);
            self.parameters = Some(
                self.controllers
                    .gateway
                    .buffer_zeroed(capacity * size_of::<f32>())
                    .map_err(metal)?,
            );
            self.capacity = capacity;
            self.used = 0;
            self.slots.clear();
        }
        let Some(buffer) = self.parameters.as_mut() else {
            return Err(CoreError::StateConflict {
                operation: "metal controller residency",
                detail: "no parameter buffer".to_owned(),
            });
        };
        for &index in &self.dispatched {
            let Some(job) = jobs.get(index) else { continue };
            let parameters = job.controller.parameters();
            let address = parameters.as_ptr() as usize;
            let hit = self
                .slots
                .get(&job.genome)
                .is_some_and(|slot| slot.address == address && slot.length == parameters.len());
            if hit {
                continue;
            }
            buffer.write_f32(self.used, parameters).map_err(metal)?;
            self.slots.insert(
                job.genome,
                Slot {
                    offset: self.used,
                    address,
                    length: parameters.len(),
                },
            );
            self.used += parameters.len();
        }
        Ok(())
    }

    /// Grow the per-organism buffers to hold `count` organisms.
    fn hold(&mut self, count: usize) -> Result<(), CoreError> {
        if count <= self.organisms && self.inputs.is_some() {
            return Ok(());
        }
        let organisms = count.next_power_of_two().max(256);
        let gateway = &self.controllers.gateway;
        let zeroed = |bytes: usize| gateway.buffer_zeroed(bytes).map_err(metal);
        self.inputs = Some(zeroed(organisms * SENSOR_WIDTH * size_of::<f32>())?);
        self.jobs = Some(zeroed(organisms * JOB_WORDS * size_of::<u32>())?);
        self.raw = Some(zeroed(organisms * ACTION_WIDTH * size_of::<f32>())?);
        self.faults = Some(zeroed(organisms * size_of::<u32>())?);
        self.organisms = organisms;
        Ok(())
    }
}

impl ControllerBackend for MetalBackend {
    fn describe(&self) -> String {
        format!("metal ({})", self.controllers.device_name())
    }

    fn forget(&mut self) {
        self.slots.clear();
        self.used = 0;
    }

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
    ) -> Result<(), CoreError> {
        outputs.clear();
        reserve(outputs, jobs.len(), "metal controller outputs")?;
        self.dispatched.clear();
        self.staged_inputs.clear();
        // A job the kernel cannot take, or whose inputs the reference
        // refuses, is deferred: the reference evaluates it, or refuses it.
        for (index, entry) in jobs.iter().enumerate() {
            let finite = entry.inputs.iter().all(|&value| (value as f32).is_finite());
            let shaped = job(
                entry.controller.topology(),
                entry.controller.parameters().len(),
                0,
            )
            .is_ok();
            if finite && shaped {
                reserve(&mut self.dispatched, 1, "metal controller dispatch")?;
                self.dispatched.push(index);
            }
        }
        if !self.dispatched.is_empty() {
            self.place(jobs)?;
            let count = self.dispatched.len();
            self.hold(count)?;
            reserve(
                &mut self.staged_inputs,
                count * SENSOR_WIDTH,
                "metal inputs",
            )?;
            self.staged_jobs.clear();
            reserve(&mut self.staged_jobs, count * JOB_WORDS, "metal jobs")?;
            for &index in &self.dispatched {
                let Some(job_entry) = jobs.get(index) else {
                    continue;
                };
                let offset = self
                    .slots
                    .get(&job_entry.genome)
                    .map(|slot| slot.offset)
                    .ok_or_else(|| CoreError::StateConflict {
                        operation: "metal controller residency",
                        detail: format!("{} is not resident", job_entry.genome),
                    })?;
                let words = job(
                    job_entry.controller.topology(),
                    job_entry.controller.parameters().len(),
                    offset,
                )?;
                self.staged_jobs.extend_from_slice(&words);
                self.staged_inputs
                    .extend(job_entry.inputs.iter().map(|&value| value as f32));
            }
            let (Some(parameters), Some(inputs), Some(job_words), Some(raw), Some(faults)) = (
                self.parameters.as_ref(),
                self.inputs.as_mut(),
                self.jobs.as_mut(),
                self.raw.as_ref(),
                self.faults.as_ref(),
            ) else {
                return Err(CoreError::StateConflict {
                    operation: "metal controller residency",
                    detail: "buffers missing".to_owned(),
                });
            };
            inputs.write_f32(0, &self.staged_inputs).map_err(metal)?;
            job_words.write_u32(0, &self.staged_jobs).map_err(metal)?;
            self.controllers
                .gateway
                .dispatch(
                    &self.controllers.forward,
                    &[parameters, inputs, job_words, raw, faults],
                    Grid::linear(count, FORWARD_THREADS),
                )
                .map_err(metal)?;
            self.values.clear();
            self.values.resize(count * ACTION_WIDTH, 0.0);
            raw.read_f32(&mut self.values).map_err(metal)?;
            self.flags.clear();
            self.flags.resize(count, 0);
            faults.read_u32(&mut self.flags).map_err(metal)?;
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
    /// The Metal backend; refused on a host without a Metal device.
    Metal,
    /// Metal where this host has a Metal device, otherwise the CPU
    /// reference.
    #[default]
    Auto,
}

impl ControllerChoice {
    /// The backend this choice selects on this host, or `None` for the CPU
    /// reference.
    ///
    /// # Errors
    /// For [`ControllerChoice::Metal`], [`MetalBackend::open`]'s errors. For
    /// [`ControllerChoice::Auto`], those other than a missing Metal device:
    /// a Mac whose kernel does not compile is a defect to report, not a
    /// reason to fall back quietly.
    pub fn backend(self) -> Result<Option<Box<dyn ControllerBackend>>, CoreError> {
        let boxed = |backend: MetalBackend| -> Box<dyn ControllerBackend> { Box::new(backend) };
        match self {
            Self::Cpu => Ok(None),
            Self::Metal => MetalBackend::open().map(boxed).map(Some),
            Self::Auto => match MetalBackend::open() {
                Ok(backend) => Ok(Some(boxed(backend))),
                Err(CoreError::MissingCapability { .. }) => Ok(None),
                Err(error) => Err(error),
            },
        }
    }
}

impl std::str::FromStr for ControllerChoice {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "cpu" => Ok(Self::Cpu),
            "metal" => Ok(Self::Metal),
            "auto" => Ok(Self::Auto),
            other => Err(format!("`{other}` is not `cpu`, `metal` or `auto`")),
        }
    }
}
