//! Fixed facts of the `field-ecology-v1` model and checked capacity
//! arithmetic (plan §6.1–§6.4, §7.5, A.2).

use crate::error::CoreError;

/// The model identity this build implements.
pub const FIELD_ECOLOGY_V1: &str = "field-ecology-v1";

/// `field-ecology-v1` with one rule changed, for the chemistry-only control
/// (plan §15.3, §21.6, H2; FL-041): every controller is evaluated, and its
/// evaluation charged, exactly as in `field-ecology-v1`, but its actions
/// are decoded from an all-zero raw output, so no organism's decisions
/// depend on what it senses or on its controller.
pub const FIELD_ECOLOGY_V1_CHEMISTRY_ONLY: &str = "field-ecology-v1-chemistry-only";

/// `field-ecology-v1` with one rule changed, for the inheritance control
/// (plan §15.3 "freshly randomized offspring", §21.6; FL-041): a newborn's
/// genome is drawn as a founder's would be at the newborn's own organism
/// identity (A.3 items 2–3), instead of mutated from its parent's. Births,
/// their energy and every other rule are unchanged.
pub const FIELD_ECOLOGY_V1_FRESH_OFFSPRING: &str = "field-ecology-v1-fresh-offspring";

/// `field-ecology-v1` with one rule changed (plan §5.4): the chemical
/// response's gain `g_u` is 1 instead of the tick's uptake ratio
/// `U / (Δt U_max)`, so an organism that absorbed nothing still follows the
/// field. Motion is still bounded by `maximum_acceleration` and
/// `maximum_speed` and paid at `c_v ‖v‖²` from what the organism holds, so a
/// starving organism can spend its last energy moving towards food.
pub const FIELD_ECOLOGY_V1_UNGATED_MOTION: &str = "field-ecology-v1-ungated-motion";

/// `field-ecology-v1` with one rule added (plan §5.4's "direct collisions,
/// adhesion … define later model variants"): every organism is a disk of
/// the `[contact]` table's diameter `D`, and after phase 8 each survivor is
/// displaced by `k/2` of its overlap with every other survivor, and pulled
/// towards each one within `R D` by an adhesion `k_a` scaled by the square
/// of their affinities' positive cosine, the sum clamped to
/// `min(D/2, ½ min(h_x, h_y))`. Contact moves positions only: no velocity,
/// energy or ledger entry changes. The model requires the `[contact]` table;
/// every other refuses it. See `flife_engine::contact`.
pub const FIELD_ECOLOGY_V1_CONTACT: &str = "field-ecology-v1-contact";

/// [`FIELD_ECOLOGY_V1_CONTACT`] with
/// [`FIELD_ECOLOGY_V1_CHEMISTRY_ONLY`]'s rule: its chemistry-only control.
pub const FIELD_ECOLOGY_V1_CONTACT_CHEMISTRY_ONLY: &str = "field-ecology-v1-contact-chemistry-only";

/// [`FIELD_ECOLOGY_V1_CONTACT`] with
/// [`FIELD_ECOLOGY_V1_FRESH_OFFSPRING`]'s rule: its inheritance control.
pub const FIELD_ECOLOGY_V1_CONTACT_FRESH_OFFSPRING: &str =
    "field-ecology-v1-contact-fresh-offspring";

/// [`FIELD_ECOLOGY_V1_CONTACT`] with contact inhibition of locomotion (plan
/// §5.4): after the contact model's displacement, `[contact] sweeps − 1`
/// further sweeps push overlapping bodies apart only, each from the
/// positions the last one left, and a survivor then loses the share
/// `[contact] inhibition` of the part of its velocity opposed to the push of
/// the bodies it overlapped, summed over its sweeps (adhesion's pull does
/// not count). It keeps the rest of its velocity, so no speed is added and
/// no energy or ledger entry changes.
/// See `flife_engine::contact`.
pub const FIELD_ECOLOGY_V1_CONTACT_INHIBITION: &str = "field-ecology-v1-contact-inhibition";

/// [`FIELD_ECOLOGY_V1_CONTACT_INHIBITION`] with
/// [`FIELD_ECOLOGY_V1_CHEMISTRY_ONLY`]'s rule: its chemistry-only control.
pub const FIELD_ECOLOGY_V1_CONTACT_INHIBITION_CHEMISTRY_ONLY: &str =
    "field-ecology-v1-contact-inhibition-chemistry-only";

/// [`FIELD_ECOLOGY_V1_CONTACT_INHIBITION`] with
/// [`FIELD_ECOLOGY_V1_FRESH_OFFSPRING`]'s rule: its inheritance control.
pub const FIELD_ECOLOGY_V1_CONTACT_INHIBITION_FRESH_OFFSPRING: &str =
    "field-ecology-v1-contact-inhibition-fresh-offspring";

/// [`FIELD_ECOLOGY_V1_CONTACT`] with cell sorting (plan §5.4): unlike
/// chemistry repels. Within adhesion's reach, the pull's weight `w²`, the
/// square of the affinities' positive cosine, becomes `(1 + r) w² − r` with
/// `r` the `[contact] repulsion`: bodies as alike as `w² > r / (1 + r)` still
/// pull, less strongly, and less alike ones push apart over the same shell,
/// most strongly `r` times adhesion's pull. Contact still moves positions
/// only. See `flife_engine::contact`.
pub const FIELD_ECOLOGY_V1_CONTACT_SORTING: &str = "field-ecology-v1-contact-sorting";

/// [`FIELD_ECOLOGY_V1_CONTACT_SORTING`] with
/// [`FIELD_ECOLOGY_V1_CHEMISTRY_ONLY`]'s rule: its chemistry-only control.
pub const FIELD_ECOLOGY_V1_CONTACT_SORTING_CHEMISTRY_ONLY: &str =
    "field-ecology-v1-contact-sorting-chemistry-only";

/// [`FIELD_ECOLOGY_V1_CONTACT_SORTING`] with
/// [`FIELD_ECOLOGY_V1_FRESH_OFFSPRING`]'s rule: its inheritance control.
pub const FIELD_ECOLOGY_V1_CONTACT_SORTING_FRESH_OFFSPRING: &str =
    "field-ecology-v1-contact-sorting-fresh-offspring";

/// The most contact sweeps a tick may run.
pub const MAXIMUM_CONTACT_SWEEPS: u32 = 16;

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

/// Maximum per-organism recurrent internal state width admitted by
/// `independent-recurrent-mlp-v1` (plan §25.2; FL-054).
pub const MAXIMUM_RECURRENT_STATE_WIDTH: u32 = 16;

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
