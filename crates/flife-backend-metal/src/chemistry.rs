//! FL-032 (`flife-kf8`), first stage: phase 6's chemistry on Metal (plan
//! §5.3, §8.2, §10.7). [`MetalChemistry::settle`] runs, one thread per
//! organism, what flife-engine's reference runs for each organism at the
//! start of phase 6: every granted packet's composition from the observed
//! field, its efficiency `η_max tanh(k a·z − b)`, the compensated sums of
//! `settle_efficiencies`, the gain credited and the toxicity paid. Every
//! value is the reference's, bit for bit (`tests/chemistry.rs` compares them
//! with `flife_engine::Chemistry::settle`).
//!
//! The device computes in `chemistry.metal` on `softfloat64.metal`'s
//! binary64. A thread that meets an input the reference would refuse, or a
//! value softfloat64 cannot carry (an infinity, which it turns into a NaN),
//! reports a status instead of an outcome, and the call is refused: the
//! caller then settles on the reference, which reports its own error or
//! finds none.
//!
//! Maintenance and motion, which follow chemistry, are [`crate::motion`]'s.
//! Not yet here (FL-032 continues): the engine port, emission and the
//! reproduction proposal, and residency. Each call copies the field and
//! every organism's affinity and packets to the device and the outcomes
//! back.

use flife_core::CoreError;
use flife_field::FieldState;
use flife_field::state::CHANNELS;
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

use crate::owner::{Binding, DeviceOwner, Kernel, Limits, Owned, Purpose, metal};

/// The chemistry kernel source, with the software binary64 ahead of it.
pub const CHEMISTRY_MSL: &str = concat!(
    include_str!("softfloat64.metal"),
    include_str!("chemistry.metal")
);

/// `chemistry.metal`'s layouts, in words.
const PARAMS: usize = 12;
const ORGANISM: usize = 2 * CHANNELS + 5;
const PACKET: usize = 3;
const OUTCOME: usize = 12;
/// Words per row of [`MetalChemistry::settle_ops`]'s results.
pub const SETTLE_OPS_WORDS: usize = 14;

/// Threads per threadgroup.
const THREADS: usize = 64;

/// The constants of `bounded-affinity-v1` the settlement uses (flife-engine's
/// `Chemistry`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChemistryConstants {
    /// `η_max ∈ (0, 1]`.
    pub maximum_efficiency: f64,
    /// The affinity gain `k ≥ 0`.
    pub gain: f64,
    /// The discrimination threshold `b > 0`.
    pub threshold: f64,
    /// The toxicity sensitivity `κ > 0`.
    pub toxicity_sensitivity: f64,
    /// The affinity norm limit, in `(0, 1]`.
    pub affinity_norm_limit: f64,
}

/// One organism's chemistry: its affinity, its budget at the start of the
/// tick, and its granted packets, `packets` of the call's packet list from
/// `first`, in the order the reference settles them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChemistryOrganism {
    /// The heritable affinity direction `a`.
    pub affinity: [f64; CHANNELS],
    /// The budget's balance.
    pub remaining: f64,
    /// Whether the budget is exhausted.
    pub exhausted: bool,
    /// The index of the organism's first packet.
    pub first: usize,
    /// How many packets it was granted.
    pub packets: usize,
}

/// One granted packet: its cell in the observed field and `u_c`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChemistryPacket {
    /// The cell, row-major as the field numbers them.
    pub cell: usize,
    /// The granted energy.
    pub granted: f64,
}

/// One organism's settlement: flife-engine's `UptakeSettlement` and the
/// budget after it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChemistryOutcome {
    /// `U = Σ_c u_c`.
    pub absorbed: f64,
    /// `G = Σ_c max(η_c, 0) u_c`.
    pub gain: f64,
    /// The requested toxicity `κ Σ_c max(−η_c, 0) u_c`.
    pub requested_toxicity: f64,
    /// The toxicity paid.
    pub paid_toxicity: f64,
    /// The budget's balance after the gain and the toxicity.
    pub remaining: f64,
    /// Whether the budget is exhausted after them.
    pub exhausted: bool,
}

fn words(value: f64) -> [u32; 2] {
    let bits = value.to_bits();
    [(bits & 0xFFFF_FFFF) as u32, (bits >> 32) as u32]
}

/// `value`'s bits as the `(low, high)` word pair at `target[at..at + 2]`.
pub(crate) fn put(target: &mut [u32], at: usize, value: f64) {
    target[at..at + 2].copy_from_slice(&words(value));
}

/// The binary64 whose bits are the `(low, high)` word pair `pair[..2]`.
pub(crate) fn value(pair: &[u32]) -> f64 {
    f64::from_bits(u64::from(pair[0]) | (u64::from(pair[1]) << 32))
}

fn overflow() -> CoreError {
    CoreError::Overflow {
        quantity: "metal chemistry size",
    }
}

/// `value` as a `u32` word, or an overflow for a call too large for one.
pub(crate) fn word(value: usize) -> Result<u32, CoreError> {
    u32::try_from(value).map_err(|_| overflow())
}

fn refuse(detail: String) -> CoreError {
    CoreError::StateConflict {
        operation: "metal chemistry",
        detail,
    }
}

/// What a nonzero status in `chemistry.metal` means.
fn status_name(status: u32) -> &'static str {
    match status {
        1 => "a non-finite tanh argument",
        2 => "a negative or non-finite grant",
        3 => "an efficiency outside [-1, 1]",
        4 => "a gain credited to an exhausted budget",
        5 => "a credit that does not stay finite",
        6 => "a non-finite or negative toxicity request",
        7 => "a cell outside the field",
        8 => "a non-finite total",
        9 => "a non-finite affinity or one past the limit",
        _ => "an unknown status",
    }
}

/// A device buffer with room for at least `capacity` elements, replaced by
/// one of the next power of two when a call outgrows it.
#[derive(Debug)]
pub(crate) struct Slot {
    owned: Option<Owned<u32>>,
    capacity: usize,
}

impl Slot {
    pub(crate) const fn new() -> Self {
        Self {
            owned: None,
            capacity: 0,
        }
    }

    pub(crate) fn ensure(
        &mut self,
        owner: &DeviceOwner,
        len: usize,
    ) -> Result<&Owned<u32>, CoreError> {
        if self.owned.is_none() || self.capacity < len {
            self.owned = None;
            let capacity = len.max(1).next_power_of_two();
            self.owned = Some(owner.zeroed_owned(capacity)?);
            self.capacity = capacity;
        }
        self.owned.as_ref().ok_or_else(overflow)
    }
}

/// The field's planes as F32 bits, kept separately so that its buffer is
/// `f32` like the field's own.
#[derive(Debug)]
struct FieldSlot {
    owned: Option<Owned<f32>>,
    capacity: usize,
}

/// Phase 6's chemistry on Metal; see the module documentation.
pub struct MetalChemistry {
    owner: DeviceOwner,
    settle: Pipeline,
    ops: Pipeline,
    field: FieldSlot,
    params: Slot,
    organisms: Slot,
    packets: Slot,
    out: Slot,
    host: Vec<u32>,
}

impl std::fmt::Debug for MetalChemistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalChemistry")
            .field("device", &self.owner.device_name())
            .finish_non_exhaustive()
    }
}

impl MetalChemistry {
    /// Open the default Metal device and compile the chemistry kernels.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] without a Metal device (always, off
    /// macOS); [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn open() -> Result<Self, CoreError> {
        Self::with_owner(DeviceOwner::open(Limits::UNBOUNDED)?)
    }

    /// The chemistry kernels on `owner`'s device; their buffers are
    /// `owner`'s.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn with_owner(owner: DeviceOwner) -> Result<Self, CoreError> {
        let library = owner.library(CHEMISTRY_MSL, MathMode::Safe)?;
        let pipeline = |name: &str| library.pipeline(name).map_err(metal);
        Ok(Self {
            settle: pipeline("chemistry_settle")?,
            ops: pipeline("settle_ops")?,
            owner,
            field: FieldSlot {
                owned: None,
                capacity: 0,
            },
            params: Slot::new(),
            organisms: Slot::new(),
            packets: Slot::new(),
            out: Slot::new(),
            host: Vec::new(),
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

    /// Settle every organism's chemistry against `observed`, the field its
    /// grants were resolved on: `out[i]` is `organisms[i]`'s settlement, as
    /// the reference computes it.
    ///
    /// # Errors
    /// - [`CoreError::NumericalInvalid`] for constants outside their ranges.
    /// - [`CoreError::LayoutMismatch`] for an organism whose packets run past
    ///   `packets`.
    /// - [`CoreError::StateConflict`] when any organism meets an input the
    ///   reference refuses or a value the device cannot carry: the caller
    ///   settles that tick on the reference. Also if the dispatch fails.
    /// - [`CoreError::Overflow`] for a call too large to describe in `u32`.
    ///
    /// On error `out` is left empty.
    pub fn settle(
        &mut self,
        observed: &FieldState,
        constants: &ChemistryConstants,
        organisms: &[ChemistryOrganism],
        packets: &[ChemistryPacket],
        out: &mut Vec<ChemistryOutcome>,
    ) -> Result<(), CoreError> {
        out.clear();
        check_constants(constants)?;
        if organisms.is_empty() {
            return Ok(());
        }
        let cells = observed.energy().len();
        let mut params = [0_u32; PARAMS];
        params[0] = word(cells)?;
        params[1] = word(organisms.len())?;
        put(&mut params, 2, constants.maximum_efficiency);
        put(&mut params, 4, constants.gain);
        put(&mut params, 6, constants.threshold);
        put(&mut params, 8, constants.toxicity_sensitivity);
        put(&mut params, 10, constants.affinity_norm_limit);

        let host = &mut self.host;
        host.clear();
        host.try_reserve(organisms.len().saturating_mul(ORGANISM))
            .map_err(|_| overflow())?;
        for organism in organisms {
            let end = organism
                .first
                .checked_add(organism.packets)
                .ok_or_else(overflow)?;
            if end > packets.len() {
                return Err(CoreError::LayoutMismatch {
                    buffer: "chemistry packets",
                    detail: format!(
                        "an organism's packets {}..{end} run past {}",
                        organism.first,
                        packets.len()
                    ),
                });
            }
            let mut row = [0_u32; ORGANISM];
            for (channel, &component) in organism.affinity.iter().enumerate() {
                put(&mut row, 2 * channel, component);
            }
            put(&mut row, 2 * CHANNELS, organism.remaining);
            row[2 * CHANNELS + 2] = u32::from(organism.exhausted);
            row[2 * CHANNELS + 3] = word(organism.first)?;
            row[2 * CHANNELS + 4] = word(organism.packets)?;
            host.extend_from_slice(&row);
        }
        let mut packet_words = Vec::new();
        packet_words
            .try_reserve(packets.len().saturating_mul(PACKET).max(PACKET))
            .map_err(|_| overflow())?;
        for packet in packets {
            let mut row = [word(packet.cell)?, 0, 0];
            put(&mut row, 1, packet.granted);
            packet_words.extend_from_slice(&row);
        }
        if packet_words.is_empty() {
            packet_words.extend_from_slice(&[0; PACKET]);
        }

        let Self {
            owner,
            settle,
            field,
            params: params_slot,
            organisms: organisms_slot,
            packets: packets_slot,
            out: out_slot,
            host,
            ..
        } = self;
        let planes = observed.as_slice();
        if field.owned.is_none() || field.capacity < planes.len() {
            field.owned = None;
            field.owned = Some(owner.zeroed_owned(planes.len())?);
            field.capacity = planes.len();
        }
        let field_buffer = field
            .owned
            .as_ref()
            .map(Owned::buffer)
            .ok_or_else(overflow)?;
        let params_buffer = params_slot.ensure(owner, PARAMS)?.buffer();
        let organisms_buffer = organisms_slot.ensure(owner, host.len())?.buffer();
        let packets_buffer = packets_slot.ensure(owner, packet_words.len())?.buffer();
        let rows = organisms.len().checked_mul(OUTCOME).ok_or_else(overflow)?;
        let out_buffer = out_slot.ensure(owner, rows)?.buffer();
        owner.write(field_buffer)?.write(0, planes)?;
        owner.write(params_buffer)?.write(0, &params)?;
        owner.write(organisms_buffer)?.write(0, host)?;
        owner.write(packets_buffer)?.write(0, &packet_words)?;
        let bindings: [Binding; 5] = [
            field_buffer.reads(),
            params_buffer.reads(),
            organisms_buffer.reads(),
            packets_buffer.reads(),
            out_buffer.writes(),
        ];
        owner.submit_batch(&[Kernel {
            pipeline: settle,
            bindings: &bindings,
            grid: Grid::linear(organisms.len().div_ceil(THREADS), THREADS),
        }])?;

        host.clear();
        host.resize(rows, 0);
        owner.read(out_buffer, Purpose::Readback)?.read(host)?;
        out.try_reserve(organisms.len()).map_err(|_| overflow())?;
        for (index, row) in host.as_chunks::<OUTCOME>().0.iter().enumerate() {
            if row[0] != 0 {
                out.clear();
                return Err(refuse(format!(
                    "organism {index} of the call met {}; the reference settles this tick",
                    status_name(row[0])
                )));
            }
            out.push(ChemistryOutcome {
                absorbed: value(&row[1..3]),
                gain: value(&row[3..5]),
                requested_toxicity: value(&row[5..7]),
                paid_toxicity: value(&row[7..9]),
                remaining: value(&row[9..11]),
                exhausted: row[11] != 0,
            });
        }
        Ok(())
    }

    /// The kernel's arithmetic on operand pairs `(a, b)` as bit patterns.
    /// Each result row holds, as `(low, high)` word pairs: `tanh(a)` (a NaN
    /// for a non-finite `a`), `add_down(a, b)`, `sub_down(a, b)`,
    /// `next_down(a)`, `max(a, b)`, `min(a, b)`, and the compensated sum of
    /// `a, b, −a, b`. `pairs.len()` must be a positive multiple of 64.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for another length;
    /// [`CoreError::StateConflict`] if the dispatch fails.
    pub fn settle_ops(
        &self,
        pairs: &[[u64; 2]],
        out: &mut Vec<[u32; SETTLE_OPS_WORDS]>,
    ) -> Result<(), CoreError> {
        if pairs.is_empty() || !pairs.len().is_multiple_of(64) {
            return Err(CoreError::LayoutMismatch {
                buffer: "settle operands",
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
            .zeroed_owned::<u32>(pairs.len() * SETTLE_OPS_WORDS)?;
        self.owner.submit(
            &self.ops,
            &[operands.buffer().reads(), results.buffer().writes()],
            Grid::linear(pairs.len() / 64, 64),
        )?;
        let mut flat = vec![0_u32; pairs.len() * SETTLE_OPS_WORDS];
        self.owner.read_back(results.buffer(), &mut flat)?;
        out.clear();
        out.extend(flat.as_chunks::<SETTLE_OPS_WORDS>().0.iter().copied());
        Ok(())
    }
}

/// flife-engine's `Chemistry::new` ranges, which the kernel takes as given.
fn check_constants(constants: &ChemistryConstants) -> Result<(), CoreError> {
    let ChemistryConstants {
        maximum_efficiency,
        gain,
        threshold,
        toxicity_sensitivity,
        affinity_norm_limit,
    } = *constants;
    let checks = [
        (
            "chemistry.maximum_efficiency",
            maximum_efficiency,
            maximum_efficiency > 0.0 && maximum_efficiency <= 1.0,
        ),
        ("chemistry.affinity_gain", gain, gain >= 0.0),
        (
            "chemistry.discrimination_threshold",
            threshold,
            threshold > 0.0,
        ),
        (
            "chemistry.toxicity_sensitivity",
            toxicity_sensitivity,
            toxicity_sensitivity > 0.0,
        ),
        (
            "chemistry.affinity_norm_limit",
            affinity_norm_limit,
            affinity_norm_limit > 0.0 && affinity_norm_limit <= 1.0,
        ),
    ];
    for (quantity, value, admissible) in checks {
        if !value.is_finite() || !admissible {
            return Err(CoreError::NumericalInvalid {
                quantity,
                detail: format!("{value} is out of range"),
            });
        }
    }
    Ok(())
}
