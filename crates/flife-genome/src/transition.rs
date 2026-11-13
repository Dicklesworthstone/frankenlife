//! Topology transition maps (plan §6.4, A.3 items 5 and 6, Appendix C.16).
//!
//! A topology change maps a parent's parameters onto the target layout
//! before any ordinary mutation. Both maps are deterministic given the raw
//! words for the weights they initialize, and every child value is written
//! explicitly. A neuron is named by its layer and index; the leading
//! neurons of a layer are the ones that carry over.
//!
//! `overlap-plus-zero-output-v1` (same depth) copies the leading overlapping
//! subnetwork, including its biases and the output biases. An added neuron
//! gets fan-in-scaled incoming weights and a zero bias, and its outgoing
//! weights are zero, so pure widening preserves the parent's function
//! before mutation. Shrinking keeps the leading neurons and does not.
//!
//! `prefix-and-identity-bridge-v1` (depth change) does not preserve the
//! function, because inserting `tanh` is not an identity:
//! - one hidden layer `h` to two `(a, b)`: the overlapping first affine map
//!   is copied; a rectangular identity bridge with zero bias joins first-layer
//!   neuron `j` to final neuron `j` when `j` was inherited (`j < h`); the old
//!   output weights from neuron `j` become those from final neuron `j`; every
//!   other output weight is zero;
//! - two hidden layers `(a, b)` to one `h`: the overlapping first affine map
//!   is copied, the middle layer is discarded, and the old output weights
//!   from final neuron `j` become those from neuron `j` when `j` was
//!   inherited (`j < a`) and existed (`j < b`); every other output weight is
//!   zero.
//!
//! In both maps output biases are copied, added first-layer neurons are
//! initialized as above, and zeroing an added neuron's outgoing influence
//! takes precedence over initializing any connection, the bridge included.

use flife_core::CoreError;
use flife_neural::{DenseTopology, LayerShape, Weight};

use crate::init::{WeightAddress, fan_in_draw};

/// The same-depth map this module implements.
pub const OVERLAP_PLUS_ZERO_OUTPUT_V1: &str = "overlap-plus-zero-output-v1";

/// The depth-change map this module implements.
pub const PREFIX_AND_IDENTITY_BRIDGE_V1: &str = "prefix-and-identity-bridge-v1";

/// The kind of a topology change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// Same depth, under `overlap-plus-zero-output-v1`.
    EqualDepth,
    /// One hidden layer to two, under `prefix-and-identity-bridge-v1`.
    Deepen,
    /// Two hidden layers to one, under `prefix-and-identity-bridge-v1`.
    Flatten,
}

impl Transition {
    /// The kind of the change from `source` to `target`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a depth outside one or two.
    pub fn between(source: &DenseTopology, target: &DenseTopology) -> Result<Self, CoreError> {
        match (source.hidden_widths().len(), target.hidden_widths().len()) {
            (1, 1) | (2, 2) => Ok(Self::EqualDepth),
            (1, 2) => Ok(Self::Deepen),
            (2, 1) => Ok(Self::Flatten),
            (from, to) => Err(CoreError::NumericalInvalid {
                quantity: "topology depth",
                detail: format!("no transition from {from} to {to} hidden layers"),
            }),
        }
    }
}

/// What one child parameter becomes.
#[derive(Debug, Clone, Copy)]
enum Entry {
    /// The parent's parameter at this offset.
    Copy(usize),
    /// A fan-in-scaled weight from this address's word.
    Draw,
    Zero,
    One,
}

/// Map `parent`, laid out as `source`, onto `target`, writing every value of
/// `child`, both in the weight dtype `W`. Words are requested in ascending
/// (layer, output, input) order, only for the incoming weights of added
/// neurons; each such draw is rounded once to `W`.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] when a buffer does not have its topology's
/// length; [`CoreError::NumericalInvalid`] for an unsupported depth change.
pub fn map_parameters<W: Weight>(
    source: &DenseTopology,
    parent: &[W],
    target: &DenseTopology,
    child: &mut [W],
    mut word: impl FnMut(WeightAddress) -> u64,
) -> Result<Transition, CoreError> {
    for (buffer, topology, name) in [(parent, source, "parent"), (&*child, target, "child")] {
        if buffer.len() != topology.parameter_count() {
            return Err(CoreError::LayoutMismatch {
                buffer: "genome parameters",
                detail: format!(
                    "the {name} has {} values for a topology of {}",
                    buffer.len(),
                    topology.parameter_count()
                ),
            });
        }
    }
    let transition = Transition::between(source, target)?;
    let from = source.layers();
    let shape = |layers: &[LayerShape], index: usize| {
        layers
            .get(index)
            .copied()
            .ok_or_else(|| CoreError::LayoutMismatch {
                buffer: "genome parameters",
                detail: format!("no layer {index}"),
            })
    };
    // Hidden widths of the parent, by layer.
    let first = shape(from, 0)?.outputs;
    let last_hidden = shape(from, from.len() - 2)?.outputs;

    for (layer, to) in target.layers().iter().enumerate() {
        let output_layer = layer + 1 == target.layers().len();
        // For each target layer: the parent layer it reads from, if any.
        let parent_layer = match (transition, layer) {
            (Transition::EqualDepth, _) | (_, 0) => Some(shape(from, layer)?),
            (Transition::Deepen, 2) | (Transition::Flatten, 1) => {
                Some(shape(from, from.len() - 1)?)
            }
            _ => None,
        };
        let weight_entry = |output: usize, input: usize| -> Result<Entry, CoreError> {
            let inherited_output = output_layer || output < parent_layer.map_or(0, |p| p.outputs);
            Ok(match (transition, layer) {
                (Transition::Deepen, 1) => {
                    if output == input && input < first {
                        Entry::One
                    } else {
                        Entry::Zero
                    }
                }
                (Transition::Deepen, 2) | (Transition::Flatten, 1) => {
                    let carried = match transition {
                        Transition::Deepen => input < first,
                        _ => input < first && input < last_hidden,
                    };
                    match parent_layer.and_then(|p| p.weight(output, input)) {
                        Some(offset) if carried => Entry::Copy(offset),
                        _ => Entry::Zero,
                    }
                }
                _ => {
                    // The same layer of the parent (equal depth, or the
                    // first layer of a depth change). Sensor inputs are
                    // always inherited; a hidden input is inherited when the
                    // parent's previous layer had that neuron.
                    let inherited_input =
                        layer == 0 || parent_layer.is_some_and(|p| input < p.inputs);
                    if !inherited_input {
                        Entry::Zero
                    } else if !inherited_output {
                        Entry::Draw
                    } else {
                        parent_layer
                            .and_then(|p| p.weight(output, input))
                            .map_or(Entry::Zero, Entry::Copy)
                    }
                }
            })
        };
        let bias_entry = |output: usize| match (transition, layer) {
            (Transition::Deepen, 1) => Entry::Zero,
            _ => parent_layer
                .and_then(|p| p.bias(output))
                .map_or(Entry::Zero, Entry::Copy),
        };

        for output in 0..to.outputs {
            for input in 0..to.inputs {
                let entry = weight_entry(output, input)?;
                let value = resolve(entry, parent, to.inputs, || {
                    word(WeightAddress {
                        layer,
                        output,
                        input,
                    })
                })?;
                write(child, to.weight(output, input), value)?;
            }
        }
        for output in 0..to.outputs {
            let value = resolve(bias_entry(output), parent, to.inputs, || 0)?;
            write(child, to.bias(output), value)?;
        }
    }
    Ok(transition)
}

fn resolve<W: Weight>(
    entry: Entry,
    parent: &[W],
    fan_in: usize,
    word: impl FnOnce() -> u64,
) -> Result<W, CoreError> {
    match entry {
        Entry::Copy(offset) => {
            parent
                .get(offset)
                .copied()
                .ok_or_else(|| CoreError::LayoutMismatch {
                    buffer: "genome parameters",
                    detail: format!("parent offset {offset} is out of range"),
                })
        }
        Entry::Draw => fan_in_draw(word(), fan_in),
        // Zero and one are exact in every dtype.
        Entry::Zero => Ok(W::round_from_f64(0.0)),
        Entry::One => Ok(W::round_from_f64(1.0)),
    }
}

fn write<W: Weight>(child: &mut [W], offset: Option<usize>, value: W) -> Result<(), CoreError> {
    let slot = offset
        .and_then(|offset| child.get_mut(offset))
        .ok_or_else(|| CoreError::LayoutMismatch {
            buffer: "genome parameters",
            detail: "a child offset is out of range".into(),
        })?;
    *slot = value;
    Ok(())
}
