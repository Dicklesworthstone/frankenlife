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
//! **Residency.** A genome's parameters are packed once, each layer's weights
//! transposed to `[inputs][outputs]` (the kernel's `[B, I, O]` with `B = 1`),
//! and kept while the genome is in every batch. A batch drops the packed sets
//! of genomes it did not contain, so memory follows the live population. As
//! [`flife_neural::ControllerJob`] requires, a resident set is reused only for
//! the same memory: the parameters' address and length.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::num::NonZeroUsize;

use flife_core::CoreError;
use flife_core::ids::GenomeId;
use flife_core::model::MAXIMUM_HIDDEN_LAYERS;
use flife_core::numerics::tanh;
use flife_neural::{ACTION_WIDTH, ControllerBackend, ControllerJob, DenseTopology, SENSOR_WIDTH};
use ft_kernel_cpu::population_affine_f32_into;

/// The fewest jobs worth handing to another thread, as for the engine's own
/// decision lanes.
pub const MINIMUM_JOBS_PER_LANE: usize = 128;

// The evaluation below is written out for zero, one or two hidden layers.
const _: () = assert!(MAXIMUM_HIDDEN_LAYERS == 2);

/// One layer of a packed parameter set: offsets into [`Packed::values`].
#[derive(Debug, Clone, Copy)]
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

/// A genome's parameters, packed for the kernel.
#[derive(Debug)]
struct Packed {
    /// The parameters it was packed from: their address and length.
    address: usize,
    length: usize,
    /// The batch that last contained it.
    batch: u64,
    layers: Vec<Layer>,
    values: Vec<f32>,
}

impl Packed {
    /// The layers of `topology` over `length` parameters, or `None` for a
    /// shape this backend does not take.
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
    /// into `layers`' order.
    fn pack(
        address: usize,
        parameters: &[f32],
        topology: &DenseTopology,
        layers: Vec<Layer>,
        batch: u64,
    ) -> Result<Self, CoreError> {
        let mut values = Vec::new();
        reserve(&mut values, parameters.len(), "cpu controller parameters")?;
        for (source, layer) in topology.layers().iter().zip(&layers) {
            let weights = parameters.get(source.weights..source.biases);
            let biases = parameters.get(source.biases..source.biases + layer.outputs);
            let (Some(weights), Some(biases)) = (weights, biases) else {
                return Err(CoreError::LayoutMismatch {
                    buffer: "controller parameters",
                    detail: format!("a layer outside {} parameters", parameters.len()),
                });
            };
            // Input `i`'s column, `weight[0][i], weight[1][i], …`, becomes
            // the packed row `i`.
            for input in 0..layer.inputs {
                values.extend(weights.iter().skip(input).step_by(layer.inputs));
            }
            values.extend_from_slice(biases);
        }
        Ok(Self {
            address,
            length: parameters.len(),
            batch,
            layers,
            values,
        })
    }

    /// The widest hidden layer.
    fn widest_hidden(&self) -> usize {
        self.layers.split_last().map_or(0, |(_, hidden)| {
            hidden.iter().map(|layer| layer.outputs).max().unwrap_or(0)
        })
    }

    /// One organism's 36 raw outputs, or `None` where the reference would
    /// refuse it.
    fn evaluate(
        &self,
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
        let Some((output, hidden)) = self.layers.split_last() else {
            return Ok(None);
        };
        let Scratch { first, second } = scratch;
        let activated = match hidden {
            [] => {
                output.apply(&self.values, &rounded, &mut raw)?;
                true
            }
            [one] => {
                let first = &mut first[..one.outputs];
                one.apply(&self.values, &rounded, first)?;
                activate(first) && {
                    output.apply(&self.values, first, &mut raw)?;
                    true
                }
            }
            [one, two] => {
                let first = &mut first[..one.outputs];
                let second = &mut second[..two.outputs];
                one.apply(&self.values, &rounded, first)?;
                activate(first) && {
                    two.apply(&self.values, first, second)?;
                    activate(second) && {
                        output.apply(&self.values, second, &mut raw)?;
                        true
                    }
                }
            }
            _ => false,
        };
        Ok((activated && raw.iter().all(|value| value.is_finite())).then_some(raw))
    }
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

/// The CPU controller backend: FrankenTorch's serial population kernel on at
/// most `lanes` threads.
#[derive(Debug)]
pub struct CpuBackend {
    lanes: NonZeroUsize,
    batch: u64,
    packed: HashMap<GenomeId, Packed>,
    scratch: Vec<Scratch>,
    /// The threads the last batch ran on.
    used: usize,
}

impl CpuBackend {
    /// A backend evaluating on at most `lanes` threads, the calling one
    /// included. Give it the world's `cpu_workers`.
    #[must_use]
    pub fn new(lanes: NonZeroUsize) -> Self {
        Self {
            lanes,
            batch: 0,
            packed: HashMap::new(),
            scratch: Vec::new(),
            used: 0,
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

    /// How many genomes' parameters are packed and resident.
    #[must_use]
    pub fn resident(&self) -> usize {
        self.packed.len()
    }

    /// Pack every job's parameters that is not resident, drop those no job
    /// uses, and give the widest hidden layer among them.
    fn place(&mut self, jobs: &[ControllerJob<'_>]) -> Result<usize, CoreError> {
        self.batch += 1;
        let batch = self.batch;
        let mut widest = 0;
        for job in jobs {
            let parameters = job.controller.parameters();
            let address = parameters.as_ptr().addr();
            let resident = self
                .packed
                .get_mut(&job.genome)
                .filter(|packed| packed.address == address && packed.length == parameters.len());
            if let Some(packed) = resident {
                packed.batch = batch;
                widest = widest.max(packed.widest_hidden());
                continue;
            }
            let topology = job.controller.topology();
            let Some(layers) = Packed::shape(topology, parameters.len()) else {
                self.packed.remove(&job.genome);
                continue;
            };
            let packed = Packed::pack(address, parameters, topology, layers, batch)?;
            widest = widest.max(packed.widest_hidden());
            self.packed.insert(job.genome, packed);
        }
        self.packed.retain(|_, packed| packed.batch == batch);
        Ok(widest)
    }
}

/// Evaluate a run of jobs into its outputs.
fn run(
    packed: &HashMap<GenomeId, Packed>,
    jobs: &[ControllerJob<'_>],
    outputs: &mut [Option<[f32; ACTION_WIDTH]>],
    scratch: &mut Scratch,
) -> Result<(), CoreError> {
    for (job, output) in jobs.iter().zip(outputs) {
        let parameters = job.controller.parameters();
        let address = parameters.as_ptr().addr();
        *output = match packed.get(&job.genome) {
            Some(set) if set.address == address && set.length == parameters.len() => {
                set.evaluate(job.inputs, scratch)?
            }
            _ => None,
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

    fn forget(&mut self) {
        self.packed.clear();
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
        let packed = &self.packed;
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
            return run(packed, head_jobs, head_outputs, head_scratch);
        }
        let (result, used) = std::thread::scope(|scope| {
            let workers: Vec<_> = runs
                .map(|((jobs, outputs), scratch)| {
                    scope.spawn(move || run(packed, jobs, outputs, scratch))
                })
                .collect();
            let used = 1 + workers.len();
            let mut result = run(packed, head_jobs, head_outputs, head_scratch);
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
