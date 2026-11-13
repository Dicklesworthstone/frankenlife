//! FL-032 (`flife-kf8`): phase 8's parameter mutation on Metal (plan §7.3,
//! §10.6, A.3 item 4). [`MetalBirth::mutate`] runs, one thread per parameter
//! of every child in the call, what flife-genome's `Mutator` runs on a
//! candidate's parameters once they hold the parent's (or the parent's
//! mapped onto another member): the replacement draw, the fan-in draw, the
//! perturbation's Bernoulli draw and Box–Muller normal, and the rounding to
//! the dtype, under `fnp-philox-semantic-v1` (`philox.metal`) with flife-core's
//! `ln`, `exp` and `sin_cos_turns` on software binary64 (`numerics.metal`).
//! Every value is the reference's, bit for bit (`tests/birth.rs`).
//!
//! The caller chooses the member, maps the parent onto it, checks and seals
//! the candidate, and charges the quota: in a world, flife-engine's host
//! through `MetalSettle`'s `SettleBackend::mutation`.

use flife_core::CoreError;
use flife_core::binary16::F16;
use flife_core::rng::{Lane, Stream};
use flife_field::settle::{MUTATION_LAYERS, MutationBirth, MutationConstants, MutationCounts};
use flife_neural::{Weight, WeightDtype};
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

use crate::chemistry::{Slot, put, word};
use crate::owner::{Binding, DeviceOwner, Kernel, Limits, Purpose, metal};

/// The birth kernel source, with the software binary64, Philox and
/// flife-core's numerics ahead of it.
pub const BIRTH_MSL: &str = concat!(
    include_str!("softfloat64.metal"),
    include_str!("philox.metal"),
    include_str!("numerics.metal"),
    include_str!("birth.metal")
);

/// `birth.metal`'s layouts, in words.
const PARAMS: usize = 13;
const BIRTH: usize = 20;
// A birth row holds `M_LAYERS` = 3 layers.
const _: () = assert!(MUTATION_LAYERS == 3);
const OPS_IN: usize = 16;
/// Words per row of [`MetalBirth::birth_ops`]'s results.
pub const BIRTH_OPS_WORDS: usize = 24;

/// Threads per threadgroup.
const THREADS: usize = 64;

fn overflow() -> CoreError {
    CoreError::Overflow {
        quantity: "metal birth size",
    }
}

/// `value`'s `(low, high)` word pair at `target[at..at + 2]`.
fn put_word(target: &mut [u32], at: usize, value: u64) {
    target[at] = value as u32;
    target[at + 1] = (value >> 32) as u32;
}

/// A birth whose layers do not tile its parameters as a dense member's do.
fn misshapen(detail: String) -> CoreError {
    CoreError::LayoutMismatch {
        buffer: "child parameters",
        detail,
    }
}

/// Whether `birth`'s layers are a dense member's: at most
/// [`MUTATION_LAYERS`], each layer's weights then biases following the
/// last, ending at its parameter count.
fn check_layers(birth: &MutationBirth) -> Result<(), CoreError> {
    if birth.depth == 0 || birth.depth > MUTATION_LAYERS {
        return Err(misshapen(format!("a member of {} layers", birth.depth)));
    }
    let mut end = 0_usize;
    for (layer, shape) in birth.layers().iter().enumerate() {
        let weights = shape
            .inputs
            .checked_mul(shape.outputs)
            .and_then(|count| count.checked_add(end))
            .ok_or_else(overflow)?;
        if shape.weights != end || shape.biases != weights {
            return Err(misshapen(format!(
                "layer {layer}'s weights at {} and biases at {}, after {end}",
                shape.weights, shape.biases
            )));
        }
        end = weights.checked_add(shape.outputs).ok_or_else(overflow)?;
    }
    if end != birth.parameters {
        return Err(misshapen(format!(
            "layers ending at {end} for {} parameters",
            birth.parameters
        )));
    }
    Ok(())
}

/// A parameter dtype the kernel carries as bits.
pub trait ParameterBits: Weight {
    /// The value of `bits` (an F16's in the low 16).
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for bits the dtype does not have.
    fn from_bits(bits: u32) -> Result<Self, CoreError>;
}

impl ParameterBits for f32 {
    fn from_bits(bits: u32) -> Result<Self, CoreError> {
        Ok(Self::from_bits(bits))
    }
}

impl ParameterBits for F16 {
    fn from_bits(bits: u32) -> Result<Self, CoreError> {
        u16::try_from(bits)
            .map(Self::from_bits)
            .map_err(|_| CoreError::StateConflict {
                operation: "metal birth",
                detail: format!("the kernel returned {bits:#x} for a binary16 parameter"),
            })
    }
}

/// Phase 8's parameter mutation on Metal; see the module documentation.
pub struct MetalBirth {
    owner: DeviceOwner,
    parameters: Pipeline,
    ops: Pipeline,
    params: Slot,
    births: Slot,
    values: Slot,
    codes: Slot,
    host: Vec<u32>,
    rows: Vec<u32>,
}

impl std::fmt::Debug for MetalBirth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalBirth")
            .field("device", &self.owner.device_name())
            .finish_non_exhaustive()
    }
}

impl MetalBirth {
    /// Open the default Metal device and compile the birth kernels.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] without a Metal device (always, off
    /// macOS); [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn open() -> Result<Self, CoreError> {
        Self::with_owner(DeviceOwner::open(Limits::UNBOUNDED)?)
    }

    /// The birth kernels on `owner`'s device; their buffers are `owner`'s.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn with_owner(owner: DeviceOwner) -> Result<Self, CoreError> {
        let library = owner.library(BIRTH_MSL, MathMode::Safe)?;
        let pipeline = |name: &str| library.pipeline(name).map_err(metal);
        Ok(Self {
            parameters: pipeline("birth_parameters")?,
            ops: pipeline("birth_ops")?,
            owner,
            params: Slot::new(),
            births: Slot::new(),
            values: Slot::new(),
            codes: Slot::new(),
            host: Vec::new(),
            rows: Vec::new(),
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

    /// Mutate every child's parameters in place under `constants`: `values`
    /// holds the children's parameters in `births`' order, each laid out as
    /// its member, and `counts[i]` is how many of `births[i]`'s were
    /// replaced and perturbed. Each value is the one `Mutator` gives that
    /// candidate, as the reference computes it.
    ///
    /// # Errors
    /// - [`CoreError::NumericalInvalid`] for a rate `Mutator` refuses.
    /// - [`CoreError::LayoutMismatch`] when `values` is not the members'
    ///   parameters, or a member's layers are not a dense member's (none,
    ///   more than [`MUTATION_LAYERS`], or not tiling its parameters).
    /// - [`CoreError::Overflow`] for a member whose neurons have no lane, as
    ///   `Lane::neuron_index` refuses them, or a call too large to describe
    ///   in `u32`.
    /// - [`CoreError::StateConflict`] if the dispatch fails.
    ///
    /// On error `values` is unchanged and `counts` is left empty.
    pub fn mutate<W: ParameterBits>(
        &mut self,
        constants: &MutationConstants,
        births: &[MutationBirth],
        values: &mut [W],
        counts: &mut Vec<MutationCounts>,
    ) -> Result<(), CoreError> {
        counts.clear();
        for (quantity, probability) in [
            (
                "mutation.replacement_probability",
                constants.replacement_probability,
            ),
            ("mutation.weight_probability", constants.weight_probability),
        ] {
            if !(0.0..=1.0).contains(&probability) {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{probability} is not a probability"),
                });
            }
        }
        let deviation = constants.weight_standard_deviation;
        if !deviation.is_finite() || deviation < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "mutation.weight_standard_deviation",
                detail: format!("{deviation} is not a standard deviation"),
            });
        }
        let rows = &mut self.rows;
        rows.clear();
        rows.try_reserve(births.len().saturating_mul(BIRTH))
            .map_err(|_| overflow())?;
        let mut total = 0_usize;
        for birth in births {
            check_layers(birth)?;
            let layers = birth.layers();
            let mut row = [0_u32; BIRTH];
            put_word(&mut row, 0, birth.parent);
            put_word(&mut row, 2, birth.tick);
            row[4] = u32::from(birth.candidate);
            row[5] = word(total)?;
            row[6] = word(birth.parameters)?;
            row[7] = word(layers.len())?;
            for (layer, shape) in layers.iter().enumerate() {
                // Every neuron's lane, as the reference asks for each.
                Lane::neuron_index(
                    layer,
                    shape.outputs.saturating_sub(1),
                    shape.inputs.saturating_sub(1),
                )?;
                let at = 8 + 4 * layer;
                row[at] = word(shape.inputs)?;
                row[at + 1] = word(shape.outputs)?;
                row[at + 2] = word(shape.weights)?;
                row[at + 3] = word(shape.biases)?;
            }
            rows.extend_from_slice(&row);
            total = total.checked_add(birth.parameters).ok_or_else(overflow)?;
        }
        if values.len() != total {
            return Err(CoreError::LayoutMismatch {
                buffer: "child parameters",
                detail: format!("{} values for members of {total}", values.len()),
            });
        }
        if births.is_empty() || total == 0 {
            counts.resize(births.len(), MutationCounts::default());
            return Ok(());
        }
        let mut params = [0_u32; PARAMS];
        params[0] = word(total)?;
        params[1] = word(births.len())?;
        params[2] = u32::from(W::DTYPE == WeightDtype::F16);
        put_word(&mut params, 3, constants.seed);
        put_word(&mut params, 5, Stream::Mutation.code());
        put(&mut params, 7, constants.replacement_probability);
        put(&mut params, 9, constants.weight_probability);
        put(&mut params, 11, deviation);

        let host = &mut self.host;
        host.clear();
        host.try_reserve(total).map_err(|_| overflow())?;
        host.extend(values.iter().map(|value| value.bits()));

        let Self {
            owner,
            parameters,
            params: params_slot,
            births: births_slot,
            values: values_slot,
            codes: codes_slot,
            host,
            rows,
            ..
        } = self;
        let params_buffer = params_slot.ensure(owner, PARAMS)?.buffer();
        let births_buffer = births_slot.ensure(owner, rows.len())?.buffer();
        let values_buffer = values_slot.ensure(owner, total)?.buffer();
        let codes_buffer = codes_slot.ensure(owner, total)?.buffer();
        owner.write(params_buffer)?.write(0, &params)?;
        owner.write(births_buffer)?.write(0, rows)?;
        owner.write(values_buffer)?.write(0, host)?;
        let bindings: [Binding; 4] = [
            params_buffer.reads(),
            births_buffer.reads(),
            values_buffer.writes(),
            codes_buffer.writes(),
        ];
        owner.submit_batch(&[Kernel {
            pipeline: parameters,
            bindings: &bindings,
            grid: Grid::linear(total.div_ceil(THREADS), THREADS),
        }])?;

        // Every code and value is read and decoded before anything is
        // written back.
        rows.clear();
        rows.resize(total, 0);
        owner.read(codes_buffer, Purpose::Readback)?.read(rows)?;
        owner.read(values_buffer, Purpose::Readback)?.read(host)?;
        let mut tallies = Vec::new();
        tallies.try_reserve(births.len()).map_err(|_| overflow())?;
        let mut start = 0_usize;
        for birth in births {
            let end = start + birth.parameters;
            let mut tally = MutationCounts::default();
            for &code in &rows[start..end] {
                match code {
                    0 => {}
                    1 => tally.replaced += 1,
                    2 => tally.perturbed += 1,
                    _ => {
                        return Err(CoreError::StateConflict {
                            operation: "metal birth",
                            detail: format!("the kernel returned code {code}"),
                        });
                    }
                }
            }
            tallies.push(tally);
            start = end;
        }
        let mut mutated = Vec::new();
        mutated.try_reserve(total).map_err(|_| overflow())?;
        for &bits in host.iter() {
            mutated.push(W::from_bits(bits)?);
        }
        values.copy_from_slice(&mutated);
        *counts = tallies;
        Ok(())
    }

    /// The kernel's arithmetic for each row `[x, y, u, w1, w2, f, h, fan_in]`:
    /// each result row holds, as `(low, high)` word pairs, whether
    /// `numerics::exp(x)` is defined, its value, `ln(y)`, `sin_cos_turns(u)`,
    /// `open_unit(w1)`, `box_muller(w1, w2)`, the binary16 bits of `f`, the
    /// binary32 bits of binary16 `h`, `init::fan_in_draw`'s value of `(w1,
    /// fan_in)` before rounding, and its binary32 and binary16 bits.
    /// `rows.len()` must be a positive multiple of 64.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for another length;
    /// [`CoreError::StateConflict`] if the dispatch fails.
    pub fn birth_ops(
        &self,
        rows: &[[u64; 8]],
        out: &mut Vec<[u32; BIRTH_OPS_WORDS]>,
    ) -> Result<(), CoreError> {
        if rows.is_empty() || !rows.len().is_multiple_of(64) {
            return Err(CoreError::LayoutMismatch {
                buffer: "birth arithmetic rows",
                detail: format!("{} rows", rows.len()),
            });
        }
        let mut words = vec![0_u32; rows.len() * OPS_IN];
        for (target, row) in words.as_chunks_mut::<OPS_IN>().0.iter_mut().zip(rows) {
            for (k, &part) in row.iter().enumerate() {
                put_word(target, 2 * k, part);
            }
        }
        let input = self.owner.upload_owned(&words)?;
        let results = self
            .owner
            .zeroed_owned::<u32>(rows.len() * BIRTH_OPS_WORDS)?;
        self.owner.submit(
            &self.ops,
            &[input.buffer().reads(), results.buffer().writes()],
            Grid::linear(rows.len() / 64, 64),
        )?;
        let mut flat = vec![0_u32; rows.len() * BIRTH_OPS_WORDS];
        self.owner.read_back(results.buffer(), &mut flat)?;
        out.clear();
        out.extend(flat.as_chunks::<BIRTH_OPS_WORDS>().0.iter().copied());
        Ok(())
    }
}
