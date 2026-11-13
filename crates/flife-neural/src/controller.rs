//! The reference controller evaluator for `independent-dense-mlp-v1` (plan
//! §6.2, §6.5, §10.1, §10.3, Appendix C.11, C.16).
//!
//! This is the scalar known-answer oracle for neural outputs. It is written
//! for clarity and a declared order of operations, not for speed;
//! accelerated routes (FL-025, FL-030) are compared against it.
//!
//! Declared arithmetic, the reference F32 profile (`accumulation_dtype =
//! "f32"`):
//! - Each of the 104 F64 sensor inputs is rounded once to F32 (nearest,
//!   ties to even).
//! - Output neuron `o` of a layer with fan-in `I` accumulates
//!   `z_o = ((b_o + w_o,0 x_0) + w_o,1 x_1) + … + w_o,I−1 x_I−1` in F32: the
//!   bias first, then the inputs in ascending index, with every product
//!   and every sum rounded to F32. There is no fused multiply-add.
//! - The hidden activation `tanh` is `flife_core::numerics::tanh` of the F32
//!   pre-activation, evaluated in F64 and rounded once to F32.
//! - The output activation is `linear`: the final affine layer's `z` is the
//!   raw output. Sigmoid and every other decoding happen once, in
//!   [`crate::actions::decode`].
//!
//! Parameter layout: layers in evaluation order, and within each layer
//! `weight[out][in]` row-major followed by `bias[out]`. That is the order of
//! torch's `parameters_to_vector`. A neuron's identity is its layer and
//! index, never its packed offset (A.3 item 5); [`LayerShape`] maps one to
//! the other.
//!
//! Every organism's controller is a view over that organism's own
//! parameters. Organisms of one topology share only the shape (§6.5,
//! INV-13).

use flife_core::memory::capacity_bytes;
use flife_core::model::{MAXIMUM_HIDDEN_LAYERS, dense_parameter_count};
use flife_core::numerics::tanh;
use flife_core::{CoreError, ValidatedConfig};

use crate::actions::ACTION_WIDTH;
use crate::sensors::SENSOR_WIDTH;
use crate::weights::{Weight, WeightDtype, Weights};

/// The controller family this module implements.
pub const INDEPENDENT_DENSE_MLP_V1: &str = "independent-dense-mlp-v1";

/// The hidden activation this module implements.
pub const TANH: &str = "tanh";

/// The output activation this module implements.
pub const LINEAR: &str = "linear";

/// The weight dtype and accumulation dtype this module implements.
pub const F32: &str = "f32";

/// One affine layer's shape and where its parameters sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerShape {
    /// Fan-in.
    pub inputs: usize,
    /// Fan-out.
    pub outputs: usize,
    /// Offset of `weight[0][0]`.
    pub weights: usize,
    /// Offset of `bias[0]`.
    pub biases: usize,
}

impl LayerShape {
    /// The offset of `weight[output][input]`.
    #[must_use]
    pub const fn weight(&self, output: usize, input: usize) -> Option<usize> {
        if output < self.outputs && input < self.inputs {
            Some(self.weights + output * self.inputs + input)
        } else {
            None
        }
    }

    /// The offset of `bias[output]`.
    #[must_use]
    pub const fn bias(&self, output: usize) -> Option<usize> {
        if output < self.outputs {
            Some(self.biases + output)
        } else {
            None
        }
    }

    /// One past this layer's last parameter.
    #[must_use]
    pub const fn end(&self) -> usize {
        self.biases + self.outputs
    }
}

/// A dense topology `104 → hidden… → 36` with one or two hidden layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenseTopology {
    hidden_widths: Vec<u32>,
    layers: Vec<LayerShape>,
    parameters: usize,
    widest_hidden: usize,
}

impl DenseTopology {
    /// The topology with these hidden widths, in evaluation order.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for no hidden layer, more than two,
    /// or a zero width; [`CoreError::Overflow`] for a parameter count past
    /// `usize`.
    pub fn new(hidden_widths: &[u32]) -> Result<Self, CoreError> {
        if hidden_widths.is_empty() || hidden_widths.len() > MAXIMUM_HIDDEN_LAYERS {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.hidden_widths",
                detail: format!(
                    "the dense catalog admits 1 to {MAXIMUM_HIDDEN_LAYERS} hidden layers, got {}",
                    hidden_widths.len()
                ),
            });
        }
        let overflow = CoreError::Overflow {
            quantity: "controller parameter count",
        };
        let total = dense_parameter_count(
            u32::try_from(SENSOR_WIDTH).map_err(|_| overflow.clone())?,
            hidden_widths,
            u32::try_from(ACTION_WIDTH).map_err(|_| overflow.clone())?,
        )?;
        let parameters = usize::try_from(total).map_err(|_| overflow.clone())?;

        // Every offset below is at most `parameters`, which fits.
        let mut layers = Vec::with_capacity(hidden_widths.len() + 1);
        let mut offset = 0;
        let mut fan_in = SENSOR_WIDTH;
        let mut widest_hidden = 0;
        for (index, width) in hidden_widths
            .iter()
            .map(|&width| usize::try_from(width).map_err(|_| overflow.clone()))
            .chain([Ok(ACTION_WIDTH)])
            .enumerate()
        {
            let width = width?;
            let biases = offset + fan_in * width;
            layers.push(LayerShape {
                inputs: fan_in,
                outputs: width,
                weights: offset,
                biases,
            });
            if index < hidden_widths.len() {
                widest_hidden = widest_hidden.max(width);
            }
            offset = biases + width;
            fan_in = width;
        }
        if offset != parameters {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller parameters",
                detail: format!("layers end at {offset}, the count is {parameters}"),
            });
        }
        Ok(Self {
            hidden_widths: hidden_widths.to_vec(),
            layers,
            parameters,
            widest_hidden,
        })
    }

    /// The default topology of a validated configuration. The topology is
    /// the same for either weight dtype ([`WeightDtype`]).
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another controller family,
    /// activation, weight dtype or accumulation dtype; otherwise as
    /// [`DenseTopology::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let controller = &config.config().controller;
        WeightDtype::named(&controller.canonical_weight_dtype)?;
        for (role, name, implemented) in [
            (
                "controller.family",
                &controller.family,
                INDEPENDENT_DENSE_MLP_V1,
            ),
            (
                "controller.hidden_activation",
                &controller.hidden_activation,
                TANH,
            ),
            (
                "controller.output_activation",
                &controller.output_activation,
                LINEAR,
            ),
            (
                "controller.accumulation_dtype",
                &controller.accumulation_dtype,
                F32,
            ),
        ] {
            if name != implemented {
                return Err(CoreError::UnknownAlgorithm {
                    role,
                    name: name.clone(),
                });
            }
        }
        Self::new(&controller.hidden_widths)
    }

    /// Hidden widths in evaluation order.
    #[must_use]
    pub fn hidden_widths(&self) -> &[u32] {
        &self.hidden_widths
    }

    /// Every affine layer in evaluation order; the last is the output layer.
    #[must_use]
    pub fn layers(&self) -> &[LayerShape] {
        &self.layers
    }

    /// `P`: weights plus biases.
    #[must_use]
    pub const fn parameter_count(&self) -> usize {
        self.parameters
    }
}

/// Reusable activation storage for evaluations. One scratch serves any
/// topology; it grows to the widest hidden layer it has seen. Nothing in
/// it survives from one evaluation into the next.
#[derive(Debug, Clone, Default)]
pub struct ControllerScratch {
    first: Vec<f32>,
    second: Vec<f32>,
}

impl ControllerScratch {
    /// Scratch sized for `topology`.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the host refuses the storage.
    pub fn new(topology: &DenseTopology) -> Result<Self, CoreError> {
        let mut scratch = Self::default();
        scratch.reserve(topology.widest_hidden)?;
        Ok(scratch)
    }

    /// Bytes its two activation buffers hold (FL-043).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        capacity_bytes(&self.first).saturating_add(capacity_bytes(&self.second))
    }

    fn reserve(&mut self, width: usize) -> Result<(), CoreError> {
        for buffer in [&mut self.first, &mut self.second] {
            buffer.clear();
            buffer
                .try_reserve_exact(width)
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "controller activations",
                    bytes: u64::try_from(width.saturating_mul(size_of::<f32>()))
                        .unwrap_or(u64::MAX),
                })?;
        }
        Ok(())
    }
}

/// One organism's controller: its own parameters, read through a topology,
/// in either weight dtype ([`Weights`]).
#[derive(Debug, Clone, Copy)]
pub struct ReferenceController<'a> {
    topology: &'a DenseTopology,
    parameters: Weights<'a>,
}

impl<'a> ReferenceController<'a> {
    /// A controller over `parameters`, laid out as `topology` declares, each
    /// finite and within `±maximum_absolute_parameter`. Within those bounds
    /// every admitted affine calculation stays finite for ABI inputs (§6.2).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonpositive or nonfinite bound,
    /// or a parameter outside it; [`CoreError::LayoutMismatch`] for a
    /// parameter count the topology does not have.
    pub fn new(
        topology: &'a DenseTopology,
        parameters: impl Into<Weights<'a>>,
        maximum_absolute_parameter: f64,
    ) -> Result<Self, CoreError> {
        let parameters = parameters.into();
        if !maximum_absolute_parameter.is_finite() || maximum_absolute_parameter <= 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.maximum_absolute_parameter",
                detail: format!("{maximum_absolute_parameter} is not a positive bound"),
            });
        }
        if parameters.len() != topology.parameters {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller parameters",
                detail: format!(
                    "{} values for a topology of {} parameters",
                    parameters.len(),
                    topology.parameters
                ),
            });
        }
        if let Some((index, value)) = parameters.iter().enumerate().find(|&(_, value)| {
            !value.is_finite() || f64::from(value.abs()) > maximum_absolute_parameter
        }) {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller parameter",
                detail: format!(
                    "parameter {index} is {value}, outside ±{maximum_absolute_parameter}"
                ),
            });
        }
        Ok(Self {
            topology,
            parameters,
        })
    }

    /// A controller over `parameters` that [`ReferenceController::new`]
    /// already admitted, under a bound no larger than the one the caller
    /// would check, and that have not changed since: a sealed genome's
    /// immutable parameters. Only the layout is checked again, so evaluating a
    /// sealed genome does not rescan every parameter each tick.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for a parameter count the topology does
    /// not have.
    pub fn sealed(
        topology: &'a DenseTopology,
        parameters: impl Into<Weights<'a>>,
    ) -> Result<Self, CoreError> {
        let parameters = parameters.into();
        if parameters.len() != topology.parameters {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller parameters",
                detail: format!(
                    "{} values for a topology of {} parameters",
                    parameters.len(),
                    topology.parameters
                ),
            });
        }
        Ok(Self {
            topology,
            parameters,
        })
    }

    /// The topology this controller is read through.
    #[must_use]
    pub const fn topology(&self) -> &'a DenseTopology {
        self.topology
    }

    /// The parameters, in the topology's layout and their canonical dtype.
    #[must_use]
    pub const fn parameters(&self) -> Weights<'a> {
        self.parameters
    }

    /// Evaluate the controller on one observation's inputs, giving the 36
    /// raw outputs of its linear final layer.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for an input that is not finite in
    /// F32, or a pre-activation that overflows;
    /// [`CoreError::AllocationFailed`] if `scratch` must grow and cannot.
    pub fn evaluate(
        &self,
        inputs: &[f64; SENSOR_WIDTH],
        scratch: &mut ControllerScratch,
    ) -> Result<[f32; ACTION_WIDTH], CoreError> {
        match self.parameters {
            Weights::F32(parameters) => self.evaluate_with(parameters, inputs, scratch),
            Weights::F16(parameters) => self.evaluate_with(parameters, inputs, scratch),
        }
    }

    /// [`ReferenceController::evaluate`] over weights of one dtype, each
    /// read as [`Weight::widen`] gives it.
    fn evaluate_with<W: Weight>(
        &self,
        parameters: &[W],
        inputs: &[f64; SENSOR_WIDTH],
        scratch: &mut ControllerScratch,
    ) -> Result<[f32; ACTION_WIDTH], CoreError> {
        let mut rounded = [0.0_f32; SENSOR_WIDTH];
        for (index, (slot, &value)) in rounded.iter_mut().zip(inputs).enumerate() {
            // Rounding each input once to F32 is the declared arithmetic.
            let narrowed = value as f32;
            if !narrowed.is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "controller input",
                    detail: format!("input {index} is {value}, not finite in F32"),
                });
            }
            *slot = narrowed;
        }

        let Some((output_layer, hidden_layers)) = self.topology.layers.split_last() else {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller parameters",
                detail: "a topology without an output layer".into(),
            });
        };
        if hidden_layers.len() > MAXIMUM_HIDDEN_LAYERS {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller parameters",
                detail: format!("{} hidden layers", hidden_layers.len()),
            });
        }
        scratch.reserve(self.topology.widest_hidden)?;
        let ControllerScratch { first, second } = scratch;

        let mut source: &[f32] = &rounded;
        for (layer, activations) in hidden_layers.iter().zip([first, second]) {
            affine(parameters, layer, source, |_, z| {
                let activated = tanh(f64::from(z)).ok_or_else(|| CoreError::NumericalInvalid {
                    quantity: "controller pre-activation",
                    detail: format!("{z} overflowed"),
                })?;
                // Rounding the activation once to F32 is the declared
                // arithmetic. The push never allocates: `reserve` sized it.
                activations.push(activated as f32);
                Ok(())
            })?;
            source = activations.as_slice();
        }

        let mut raw = [0.0_f32; ACTION_WIDTH];
        affine(parameters, output_layer, source, |output, z| {
            if !z.is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "controller output",
                    detail: format!("output {output} is {z}"),
                });
            }
            if let Some(slot) = raw.get_mut(output) {
                *slot = z;
            }
            Ok(())
        })?;
        Ok(raw)
    }

    /// [`ReferenceController::evaluate`], keeping each hidden layer's
    /// activations: the inspector's sampled activations (FL-021, plan
    /// §14.2). The outputs are `evaluate`'s, bit for bit; the activations
    /// are the F32 values the next layer read.
    ///
    /// # Errors
    /// As [`ReferenceController::evaluate`], and
    /// [`CoreError::AllocationFailed`] when the copies cannot be stored.
    pub fn trace(
        &self,
        inputs: &[f64; SENSOR_WIDTH],
        scratch: &mut ControllerScratch,
    ) -> Result<Trace, CoreError> {
        let outputs = self.evaluate(inputs, scratch)?;
        let layers = self.topology.layers.len().saturating_sub(1);
        let mut hidden = Vec::new();
        for activations in [&scratch.first, &scratch.second].into_iter().take(layers) {
            let mut copy = Vec::new();
            copy.try_reserve_exact(activations.len())
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "traced activations",
                    bytes: u64::try_from(activations.len().saturating_mul(size_of::<f32>()))
                        .unwrap_or(u64::MAX),
                })?;
            copy.extend_from_slice(activations);
            hidden.push(copy);
        }
        Ok(Trace { hidden, outputs })
    }
}

/// One evaluation with its hidden activations
/// ([`ReferenceController::trace`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    /// Each hidden layer's activations, in evaluation order: `tanh` of its
    /// pre-activations, rounded once to F32.
    pub hidden: Vec<Vec<f32>>,
    /// The 36 raw outputs of the linear final layer.
    pub outputs: [f32; ACTION_WIDTH],
}

/// The raw outputs as the F64 values [`crate::actions::decode`] reads.
/// Widening is exact.
#[must_use]
pub fn widen(raw: &[f32; ACTION_WIDTH]) -> [f64; ACTION_WIDTH] {
    raw.map(f64::from)
}

/// One affine layer in the declared order: for each output neuron, the
/// bias, then each input's product in ascending index, all in F32 with each
/// weight read as [`Weight::widen`] gives it. `emit` receives each neuron's
/// `z` in output order.
fn affine<W: Weight>(
    parameters: &[W],
    layer: &LayerShape,
    input: &[f32],
    mut emit: impl FnMut(usize, f32) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    let mismatch = |detail: String| CoreError::LayoutMismatch {
        buffer: "controller parameters",
        detail,
    };
    if input.len() != layer.inputs || layer.inputs == 0 {
        return Err(mismatch(format!(
            "a layer of fan-in {} read {} activations",
            layer.inputs,
            input.len()
        )));
    }
    let weights = parameters
        .get(layer.weights..layer.biases)
        .ok_or_else(|| mismatch(format!("weights {}..{}", layer.weights, layer.biases)))?;
    let biases = parameters
        .get(layer.biases..layer.end())
        .ok_or_else(|| mismatch(format!("biases {}..{}", layer.biases, layer.end())))?;
    for (output, (row, &bias)) in weights.chunks_exact(layer.inputs).zip(biases).enumerate() {
        let mut z = bias.widen();
        for (&weight, &value) in row.iter().zip(input) {
            z += weight.widen() * value;
        }
        emit(output, z)?;
    }
    Ok(())
}
