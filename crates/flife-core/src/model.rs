//! Fixed facts of the `field-ecology-v1` model and checked capacity
//! arithmetic (plan §6.1–§6.4, §7.5, A.2).

use crate::error::CoreError;

/// The model identity this build implements.
pub const FIELD_ECOLOGY_V1: &str = "field-ecology-v1";

/// Chemical moment channels per field cell (§4.2).
pub const MOMENT_CHANNELS: u32 = 32;

/// Intrinsic sensor inputs: internal energy, age, vx, vy and recent energy
/// change (§6.1).
pub const INTRINSIC_INPUTS: u32 = 5;

/// Controller inputs: energy and every channel's value, x derivative and
/// y derivative, plus the intrinsic inputs: `3 (1 + 32) + 5 = 104` (§6.1).
pub const SENSOR_WIDTH: u32 = 3 * (1 + MOMENT_CHANNELS) + INTRINSIC_INPUTS;

/// Raw controller outputs (§6.3).
pub const ACTION_WIDTH: u32 = 36;

/// Hidden layers the dense catalog admits: one or two (§6.4).
pub const MAXIMUM_HIDDEN_LAYERS: usize = 2;

/// Parameters, weights plus biases, of a dense network
/// `input → hidden… → output` (§6.2).
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a zero width;
/// [`CoreError::Overflow`] past `u64::MAX`.
pub fn dense_parameter_count(input: u32, hidden: &[u32], output: u32) -> Result<u64, CoreError> {
    let mut total: u64 = 0;
    let mut fan_in = u64::from(input);
    for &width in hidden.iter().chain(std::iter::once(&output)) {
        if fan_in == 0 || width == 0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "layer width",
                detail: "a dense layer width is zero".into(),
            });
        }
        let fan_out = u64::from(width);
        let layer = fan_in
            .checked_mul(fan_out)
            .and_then(|weights| weights.checked_add(fan_out))
            .ok_or(CoreError::Overflow {
                quantity: "parameter count",
            })?;
        total = total.checked_add(layer).ok_or(CoreError::Overflow {
            quantity: "parameter count",
        })?;
        fan_in = fan_out;
    }
    Ok(total)
}

/// Bytes for `organisms` genomes of `parameters` each at `dtype_bytes`
/// (the fixed logical genome charge of §7.5; deduplication never lowers it).
///
/// # Errors
/// [`CoreError::Overflow`] past `u64::MAX`.
pub fn genome_bank_bytes(
    organisms: u64,
    parameters: u64,
    dtype_bytes: u64,
) -> Result<u64, CoreError> {
    organisms
        .checked_mul(parameters)
        .and_then(|values| values.checked_mul(dtype_bytes))
        .ok_or(CoreError::Overflow {
            quantity: "genome bank bytes",
        })
}
