//! `fan-in-scaled-symmetric-v1`, the founders' parameters (plan §7.2, A.3
//! items 2 and 3).
//!
//! Every weight of a layer with fan-in `I` is `(2u − 1) √(3/I)` for a
//! half-open uniform `u = k 2^-52`, where `k` is the leading 52 bits of the
//! weight's own raw random word. That is uniform on `[−√(3/I), √(3/I))` with
//! variance `1/I`. It is computed in F64 (`2u − 1` is exact, `3/I` and the
//! square root are correctly rounded) and rounded once to F32. Every bias
//! is zero.
//!
//! The raw words come from the caller, addressed by [`WeightAddress`]; this
//! module is the deterministic map from words to parameters. Founders take
//! their words from `fnp-philox-semantic-v1` ([`crate::founder`](mod@crate::founder)), and
//! mutation and transition maps from the mutation stream
//! ([`crate::mutation`]).

use flife_core::CoreError;
use flife_core::rng::half_open_unit;
use flife_neural::DenseTopology;

/// The initializer this module implements.
pub const FAN_IN_SCALED_SYMMETRIC_V1: &str = "fan-in-scaled-symmetric-v1";

/// One weight's name: its layer and the neurons it joins. This is what a
/// random address names, never a packed offset (A.3 item 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WeightAddress {
    /// The affine layer, in evaluation order.
    pub layer: usize,
    /// The receiving neuron.
    pub output: usize,
    /// The sending neuron (or sensor input, for the first layer).
    pub input: usize,
}

/// The fan-in-scaled weight a raw word gives a layer of fan-in `fan_in`.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a zero fan-in.
pub fn fan_in_weight(word: u64, fan_in: usize) -> Result<f32, CoreError> {
    if fan_in == 0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "layer fan-in",
            detail: "a zero fan-in has no initializer scale".into(),
        });
    }
    let limit = (3.0 / fan_in as f64).sqrt();
    // Rounding once to F32 is the declared canonical rounding.
    Ok(((2.0 * half_open_unit(word) - 1.0) * limit) as f32)
}

/// Fill `parameters`, laid out as `topology`, with a founder's values:
/// each weight from `word(address)` and each bias zero. Words are requested
/// in ascending (layer, output, input) order; each depends only on its
/// address.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] when `parameters` does not have the
/// topology's length.
pub fn initialize(
    topology: &DenseTopology,
    parameters: &mut [f32],
    mut word: impl FnMut(WeightAddress) -> u64,
) -> Result<(), CoreError> {
    if parameters.len() != topology.parameter_count() {
        return Err(CoreError::LayoutMismatch {
            buffer: "genome parameters",
            detail: format!(
                "{} values for a topology of {} parameters",
                parameters.len(),
                topology.parameter_count()
            ),
        });
    }
    for (layer, shape) in topology.layers().iter().enumerate() {
        if shape.inputs == 0 {
            return Err(CoreError::LayoutMismatch {
                buffer: "genome parameters",
                detail: format!("layer {layer} has no inputs"),
            });
        }
        let (weights, biases) = parameters
            .get_mut(shape.weights..shape.end())
            .ok_or_else(|| CoreError::LayoutMismatch {
                buffer: "genome parameters",
                detail: format!("layer {layer} ends past the buffer"),
            })?
            .split_at_mut(shape.biases - shape.weights);
        for (output, row) in weights.chunks_exact_mut(shape.inputs).enumerate() {
            for (input, slot) in row.iter_mut().enumerate() {
                *slot = fan_in_weight(
                    word(WeightAddress {
                        layer,
                        output,
                        input,
                    }),
                    shape.inputs,
                )?;
            }
        }
        biases.fill(0.0);
    }
    Ok(())
}
