//! Bounded per-packet chemistry (plan §5.3, §11.3, Appendix C.2, C.3, C.9).
//!
//! `bounded-affinity-v1`: an organism with affinity `a`, where
//! `||a||₂ ≤ affinity_norm_limit ≤ 1`, converts a packet of composition `z`
//! with efficiency `η(z) = η_max tanh(k a·z − b)`. A positive efficiency is
//! useful uptake and a negative one is harm.
//!
//! Under `per-granted-cell-packet` the efficiency is evaluated separately on
//! each actual granted cell packet `u_c`:
//! - useful gain `G = Σ_c max(η_c, 0) u_c`;
//! - requested toxicity `T_req = κ Σ_c max(−η_c, 0) u_c`;
//! - actual toxicity `T = min(T_req, E + G)`.
//!
//! The organism gains `G − T` and the ledger dissipates `U − G + T`, using
//! the actual clipped toxicity (C.2). Chemistry is never evaluated on the
//! request or on the mixture of granted packets: the nonlinearity and its
//! sign split do not commute with averaging (C.9).
//!
//! A packet's composition is its cell's `z_c = m_c / e_c` in the observation
//! snapshot, taken in F64 from the exact F32 values before the uptake debit.
//! A cell with `m = 0` or no energy has the neutral composition `z = 0`
//! (C.3). The neutral composition is not special-cased beyond that: with
//! `b > 0` it is mildly harmful.

use flife_core::ledger::{CompensatedSum, EnergyBudget, Ledger, Payment};
use flife_core::memory::bytes_for;
use flife_core::numerics::symplectic_rotate_channels;
use flife_core::{CoreError, Flow, Transfer, ValidatedConfig};
use flife_field::FieldState;
use flife_field::numerics::{tanh, tanh_4};
use flife_field::settle::ChemistryConstants;
use flife_field::state::CHANNELS;
use flife_genome::Traits;

use crate::world::{CorpseDirection, V2Rules};

#[cold]
#[inline(never)]
fn err_nonfinite_dot(dot: f64) -> CoreError {
    CoreError::NumericalInvalid {
        quantity: "packet composition",
        detail: format!("a·z = {dot} is not finite"),
    }
}

#[cold]
#[inline(never)]
fn err_comp_oob(comp_idx: u32, len: usize) -> CoreError {
    CoreError::LayoutMismatch {
        buffer: "packet compositions",
        detail: format!("composition index {comp_idx} out of bounds for {len}"),
    }
}

#[cold]
#[inline(never)]
fn err_scored_alloc(count: usize) -> CoreError {
    CoreError::AllocationFailed {
        buffer: "scored packets",
        bytes: bytes_for::<(f64, f64)>(count),
    }
}

#[cold]
#[inline(never)]
fn err_invalid_kappa(toxicity_sensitivity: f64) -> CoreError {
    CoreError::NumericalInvalid {
        quantity: "chemistry.toxicity_sensitivity",
        detail: format!("{toxicity_sensitivity} is out of range"),
    }
}

#[cold]
#[inline(never)]
fn err_invalid_grant(granted: f64) -> CoreError {
    CoreError::NumericalInvalid {
        quantity: "granted packet",
        detail: format!("{granted} is not a finite nonnegative energy"),
    }
}

#[cold]
#[inline(never)]
fn err_invalid_efficiency(efficiency: f64) -> CoreError {
    CoreError::NumericalInvalid {
        quantity: "packet efficiency",
        detail: format!("{efficiency} is outside [−1, 1]"),
    }
}

/// The chemistry family this module implements.
pub const BOUNDED_AFFINITY_V1: &str = "bounded-affinity-v1";

/// The chemistry evaluation this module implements.
pub const PER_GRANTED_CELL_PACKET: &str = "per-granted-cell-packet";

/// The per-run chemistry constants of `bounded-affinity-v1` (§5.3: fixed
/// for a baseline run, not heritable).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chemistry {
    maximum_efficiency: f64,
    gain: f64,
    threshold: f64,
    toxicity_sensitivity: f64,
    affinity_norm_limit: f64,
}

impl Chemistry {
    /// Constants `η_max ∈ (0, 1]`, gain `k ≥ 0`, discrimination threshold
    /// `b > 0`, toxicity sensitivity `κ > 0` and affinity norm limit in
    /// `(0, 1]`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a value outside its range.
    pub fn new(
        maximum_efficiency: f64,
        gain: f64,
        threshold: f64,
        toxicity_sensitivity: f64,
        affinity_norm_limit: f64,
    ) -> Result<Self, CoreError> {
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
        Ok(Self {
            maximum_efficiency,
            gain,
            threshold,
            toxicity_sensitivity,
            affinity_norm_limit,
        })
    }

    /// The constants of a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] when the configuration names another
    /// chemistry family or evaluation; otherwise as [`Chemistry::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let chemistry = &config.config().chemistry;
        if chemistry.family != BOUNDED_AFFINITY_V1 {
            return Err(CoreError::UnknownAlgorithm {
                role: "chemistry.family",
                name: chemistry.family.clone(),
            });
        }
        if chemistry.chemistry_evaluation != PER_GRANTED_CELL_PACKET {
            return Err(CoreError::UnknownAlgorithm {
                role: "chemistry.chemistry_evaluation",
                name: chemistry.chemistry_evaluation.clone(),
            });
        }
        Self::new(
            chemistry.maximum_efficiency,
            chemistry.affinity_gain,
            chemistry.discrimination_threshold,
            chemistry.toxicity_sensitivity,
            chemistry.affinity_norm_limit,
        )
    }

    /// `κ`.
    #[must_use]
    pub const fn toxicity_sensitivity(&self) -> f64 {
        self.toxicity_sensitivity
    }

    /// The largest admitted affinity norm.
    #[must_use]
    pub const fn affinity_norm_limit(&self) -> f64 {
        self.affinity_norm_limit
    }

    /// These constants as a device's phase 6 takes them (FL-032).
    pub(crate) const fn settle_constants(&self) -> ChemistryConstants {
        ChemistryConstants {
            maximum_efficiency: self.maximum_efficiency,
            gain: self.gain,
            threshold: self.threshold,
            toxicity_sensitivity: self.toxicity_sensitivity,
            affinity_norm_limit: self.affinity_norm_limit,
        }
    }

    /// `η(z) = η_max tanh(k a·z − b)`, with the dot product summed in channel
    /// order.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite composition.
    #[inline]
    pub fn efficiency(
        &self,
        affinity: &Affinity,
        composition: &[f64; CHANNELS],
    ) -> Result<f64, CoreError> {
        self.efficiency_slice(&affinity.0, composition)
    }

    #[inline]
    fn efficiency_slice(
        &self,
        aff: &[f64; CHANNELS],
        composition: &[f64; CHANNELS],
    ) -> Result<f64, CoreError> {
        let mut dot = 0.0;
        for (a, z) in aff.iter().zip(composition) {
            dot += a * z;
        }
        let response =
            tanh(self.gain * dot - self.threshold).ok_or_else(|| err_nonfinite_dot(dot))?;
        Ok(self.maximum_efficiency * response)
    }

    /// Settle an organism's granted `packets` against its `budget`:
    /// evaluate `η` on each packet, then settle as
    /// [`settle_efficiencies`].
    ///
    /// # Errors
    /// As [`Chemistry::efficiency`] and [`settle_efficiencies`], or
    /// [`CoreError::AllocationFailed`] if staging more than four packets fails.
    pub fn settle(
        &self,
        affinity: &Affinity,
        packets: &[ChemicalPacket],
        budget: &mut EnergyBudget,
    ) -> Result<UptakeSettlement, CoreError> {
        let mut efficiencies = [(0.0, 0.0); 4];
        let mut scored = Vec::new();
        let list: &mut [(f64, f64)] = if packets.len() == 4 {
            let (mut d0, mut d1, mut d2, mut d3) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
            let aff = &affinity.0;
            let c0 = &packets[0].composition;
            let c1 = &packets[1].composition;
            let c2 = &packets[2].composition;
            let c3 = &packets[3].composition;
            for ch in 0..CHANNELS {
                let a = aff[ch];
                d0 += a * c0[ch];
                d1 += a * c1[ch];
                d2 += a * c2[ch];
                d3 += a * c3[ch];
            }
            let args = [
                self.gain * d0 - self.threshold,
                self.gain * d1 - self.threshold,
                self.gain * d2 - self.threshold,
                self.gain * d3 - self.threshold,
            ];
            let responses = match tanh_4(args) {
                Some(r) => r,
                None => [
                    tanh(args[0]).ok_or_else(|| err_nonfinite_dot(d0))?,
                    tanh(args[1]).ok_or_else(|| err_nonfinite_dot(d1))?,
                    tanh(args[2]).ok_or_else(|| err_nonfinite_dot(d2))?,
                    tanh(args[3]).ok_or_else(|| err_nonfinite_dot(d3))?,
                ],
            };
            let max_eff = self.maximum_efficiency;
            efficiencies = [
                (packets[0].granted, max_eff * responses[0]),
                (packets[1].granted, max_eff * responses[1]),
                (packets[2].granted, max_eff * responses[2]),
                (packets[3].granted, max_eff * responses[3]),
            ];
            &mut efficiencies
        } else if packets.len() <= efficiencies.len() {
            for (slot, packet) in efficiencies.iter_mut().zip(packets) {
                *slot = (
                    packet.granted,
                    self.efficiency(affinity, &packet.composition)?,
                );
            }
            &mut efficiencies[..packets.len()]
        } else {
            scored
                .try_reserve_exact(packets.len())
                .map_err(|_| err_scored_alloc(packets.len()))?;
            for packet in packets {
                scored.push((
                    packet.granted,
                    self.efficiency(affinity, &packet.composition)?,
                ));
            }
            &mut scored
        };
        settle_efficiencies(self.toxicity_sensitivity, list, budget)
    }

    /// Zero-copy counterpart to [`Chemistry::settle`] where each packet holds
    /// `(granted, composition_index)` into a deduplicated per-cell `compositions`
    /// table, eliminating per-packet 256-byte composition copies while computing
    /// bit-identical efficiencies and settlement.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if any `composition_index` is out of bounds;
    /// otherwise as [`Chemistry::settle`].
    #[inline]
    pub(crate) fn settle_indexed(
        &self,
        affinity: &Affinity,
        packets: &[(f64, u32)],
        compositions: &[[f64; CHANNELS]],
        budget: &mut EnergyBudget,
    ) -> Result<UptakeSettlement, CoreError> {
        self.settle_indexed_slice(&affinity.0, packets, compositions, budget)
    }

    /// Borrowed-slice counterpart to [`Chemistry::settle_indexed`] avoiding
    /// 256-byte affinity stack copies when the organism's affinity is already
    /// validated and unrotated.
    ///
    /// # Errors
    /// As [`Chemistry::settle_indexed`].
    #[inline]
    pub(crate) fn settle_indexed_slice(
        &self,
        aff: &[f64; CHANNELS],
        packets: &[(f64, u32)],
        compositions: &[[f64; CHANNELS]],
        budget: &mut EnergyBudget,
    ) -> Result<UptakeSettlement, CoreError> {
        let mut efficiencies = [(0.0, 0.0); 4];
        let mut scored = Vec::new();
        let list: &mut [(f64, f64)] = if packets.len() == 4
            && let (Some(c0), Some(c1), Some(c2), Some(c3)) = (
                compositions.get(packets[0].1 as usize),
                compositions.get(packets[1].1 as usize),
                compositions.get(packets[2].1 as usize),
                compositions.get(packets[3].1 as usize),
            ) {
            // Fused 4-packet channel-order dot product + 4-lane SIMD `tanh_4`:
            // reads `aff` once and computes bit-identical IEEE-754 sums
            // and `tanh` evaluations as four sequential `efficiency` calls.
            let (mut d0, mut d1, mut d2, mut d3) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
            for ch in 0..CHANNELS {
                let a = aff[ch];
                d0 += a * c0[ch];
                d1 += a * c1[ch];
                d2 += a * c2[ch];
                d3 += a * c3[ch];
            }
            let args = [
                self.gain * d0 - self.threshold,
                self.gain * d1 - self.threshold,
                self.gain * d2 - self.threshold,
                self.gain * d3 - self.threshold,
            ];
            let responses = match tanh_4(args) {
                Some(r) => r,
                None => [
                    tanh(args[0]).ok_or_else(|| err_nonfinite_dot(d0))?,
                    tanh(args[1]).ok_or_else(|| err_nonfinite_dot(d1))?,
                    tanh(args[2]).ok_or_else(|| err_nonfinite_dot(d2))?,
                    tanh(args[3]).ok_or_else(|| err_nonfinite_dot(d3))?,
                ],
            };
            let max_eff = self.maximum_efficiency;
            efficiencies = [
                (packets[0].0, max_eff * responses[0]),
                (packets[1].0, max_eff * responses[1]),
                (packets[2].0, max_eff * responses[2]),
                (packets[3].0, max_eff * responses[3]),
            ];
            &mut efficiencies
        } else if packets.len() <= efficiencies.len() {
            for (slot, &(granted, comp_idx)) in efficiencies.iter_mut().zip(packets) {
                let comp = compositions
                    .get(comp_idx as usize)
                    .ok_or_else(|| err_comp_oob(comp_idx, compositions.len()))?;
                *slot = (granted, self.efficiency_slice(aff, comp)?);
            }
            &mut efficiencies[..packets.len()]
        } else {
            scored
                .try_reserve_exact(packets.len())
                .map_err(|_| err_scored_alloc(packets.len()))?;
            for &(granted, comp_idx) in packets {
                let comp = compositions
                    .get(comp_idx as usize)
                    .ok_or_else(|| err_comp_oob(comp_idx, compositions.len()))?;
                scored.push((granted, self.efficiency_slice(aff, comp)?));
            }
            &mut scored
        };
        settle_efficiencies(self.toxicity_sensitivity, list, budget)
    }
}

/// A heritable affinity direction `a` with `||a||₂` at most the run's limit,
/// checked in F64 (§5.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affinity([f64; CHANNELS]);

impl Affinity {
    /// The affinity `direction`, which must lie within `chemistry`'s norm
    /// limit.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite component or a norm
    /// above the limit.
    pub fn new(direction: [f64; CHANNELS], chemistry: &Chemistry) -> Result<Self, CoreError> {
        let mut squared = 0.0;
        for component in direction {
            if !component.is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "affinity direction",
                    detail: format!("component {component} is not finite"),
                });
            }
            squared += component * component;
        }
        let limit = chemistry.affinity_norm_limit;
        if squared > limit * limit {
            return Err(CoreError::NumericalInvalid {
                quantity: "affinity direction",
                detail: format!("||a||² = {squared} exceeds the limit {limit}²"),
            });
        }
        Ok(Self(direction))
    }

    /// Construct from traits already verified against a bound at or below
    /// `chemistry.affinity_norm_limit`.
    #[must_use]
    pub(crate) const fn trusted(direction: [f64; CHANNELS]) -> Self {
        Self(direction)
    }

    /// The direction.
    #[must_use]
    pub const fn direction(&self) -> &[f64; CHANNELS] {
        &self.0
    }
}

/// One granted cell packet as chemistry sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChemicalPacket {
    /// `u_c`, the granted energy.
    pub granted: f64,
    /// `z_c`, the cell's composition when the grant was resolved.
    pub composition: [f64; CHANNELS],
}

/// The composition `z_c = m_c / e_c` of `cell` in `field`, in F64 from the
/// exact F32 values. An empty cell is neutral. The moments are read in one
/// pass ([`FieldState::cell_at`]): phases 6 and 7 compute one composition
/// per granted packet.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] when `cell` is not in `field`.
pub fn composition(field: &FieldState, cell: usize) -> Result<[f64; CHANNELS], CoreError> {
    let missing = || CoreError::LayoutMismatch {
        buffer: "field",
        detail: format!("cell {cell} is not in this field"),
    };
    let energy = f64::from(*field.energy().get(cell).ok_or_else(missing)?);
    let mut z = [0.0; CHANNELS];
    if energy > 0.0 {
        let values = field.cell_at(cell).ok_or_else(missing)?;
        for (slot, &moment) in z.iter_mut().zip(&values.moments) {
            *slot = f64::from(moment) / energy;
        }
    }
    Ok(z)
}

/// One organism's uptake settlement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UptakeSettlement {
    /// `U = Σ_c u_c`, the energy the field gave up.
    pub absorbed: f64,
    /// `G = Σ_c max(η_c, 0) u_c`.
    pub gain: f64,
    /// Requested toxicity `T_req` and actual toxicity `T = min(T_req, E + G)`.
    pub toxicity: Payment,
}

impl UptakeSettlement {
    /// `G − T`, the organism's net change.
    #[must_use]
    pub fn net(&self) -> f64 {
        self.gain - self.toxicity.paid
    }

    /// `U − G + T`, the dissipation.
    #[must_use]
    pub fn dissipated(&self) -> f64 {
        (self.absorbed - self.gain) + self.toxicity.paid
    }

    /// Record this settlement: an uptake transfer of `U` that credits `G`
    /// (its loss `U − G` is charged to `uptake-inefficiency`), and the actual
    /// toxicity `T`.
    ///
    /// # Errors
    /// As [`Ledger::transfer`] and [`Ledger::record`].
    #[inline]
    pub fn record(&self, ledger: &mut impl Ledger) -> Result<(), CoreError> {
        ledger.transfer(Transfer::Uptake, self.absorbed, self.gain)?;
        ledger.record(Flow::Toxicity, self.toxicity.paid)
    }
}

/// Settle granted packets `(u_c, η_c)` against `budget` with toxicity
/// sensitivity `κ`. The gain `G` is credited first and the requested
/// toxicity is then paid, so the budget bounds it by `E + G` (§8.2: an
/// organism alive at the start of a tick absorbs before it pays). Sums run
/// in packet order and are compensated.
///
/// # Errors
/// - [`CoreError::NumericalInvalid`] for a negative or nonfinite grant, an
///   efficiency outside `[−1, 1]`, or a nonpositive or nonfinite `κ`.
/// - [`CoreError::StateConflict`] when the budget is already exhausted and
///   there is a gain to credit.
#[inline]
pub fn settle_efficiencies(
    toxicity_sensitivity: f64,
    packets: &[(f64, f64)],
    budget: &mut EnergyBudget,
) -> Result<UptakeSettlement, CoreError> {
    if !toxicity_sensitivity.is_finite() || toxicity_sensitivity <= 0.0 {
        return Err(err_invalid_kappa(toxicity_sensitivity));
    }
    let (mut absorbed, mut gain, mut harm) = (
        CompensatedSum::ZERO,
        CompensatedSum::ZERO,
        CompensatedSum::ZERO,
    );
    for &(granted, efficiency) in packets {
        if !granted.is_finite() || granted < 0.0 {
            return Err(err_invalid_grant(granted));
        }
        if !(-1.0..=1.0).contains(&efficiency) {
            return Err(err_invalid_efficiency(efficiency));
        }
        absorbed.add(granted);
        gain.add(efficiency.max(0.0) * granted);
        harm.add((-efficiency).max(0.0) * granted);
    }
    let gain = gain.value();
    let requested_toxicity = toxicity_sensitivity * harm.value();
    if gain > 0.0 {
        budget.credit(gain)?;
    }
    let toxicity = budget.pay(requested_toxicity)?;
    Ok(UptakeSettlement {
        absorbed: absorbed.value(),
        gain,
        toxicity,
    })
}

/// Effective chemical affinity `a_eff` of an organism (`V2-11` / `V2-12` seam).
///
/// Returns `*traits.affinity()` when `epigenetic-recurrence` or
/// `affinity_rotation_enabled` is disabled, or when `recurrent_state_h0 == 0.0`.
/// When active, rotates the active Carrier-0 channels (`0..active_carrier0_channels`,
/// `32` single-carrier or `24` under `V2-15`) in the `(a, J a)` symplectic plane
/// by `(π / 2) · h_0` via [`flife_neural::rotate_epigenetic_affinity`]
/// (guarding negative `h_0` through `normalize_turns(0.25 * h_0)` and enforcing
/// exact left-fold `‖a_eff‖₂² <= affinity_norm_limit²`).
///
/// Uptake (CPU Reference and `SettleBackend` `ChemistryOrganism`), corpse lysis
/// (`-a_eff` / `J a_eff`), obligate symplectic waste (`J a_eff`), and
/// [`LocalChemistry`](crate::LocalChemistry) all share this entry point so the
/// 2nd-Law rule `a_eff · J a_eff == 0` holds for every cell type.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] if `recurrent_state_h0` is nonfinite or
/// outside `[-1, 1]`, or as [`flife_neural::rotate_epigenetic_affinity`].
pub fn effective_affinity(
    traits: &Traits,
    recurrent_state_h0: f32,
    rules: &V2Rules,
    affinity_norm_limit: f64,
) -> Result<[f64; CHANNELS], CoreError> {
    if !rules.affinity_rotation_enabled() || recurrent_state_h0 == 0.0 {
        return Ok(*traits.affinity());
    }
    flife_neural::rotate_epigenetic_affinity(
        traits.affinity(),
        recurrent_state_h0,
        rules.active_carrier0_channels(),
        affinity_norm_limit,
    )
}

/// Chemical direction deposited by a lysed corpse (`V2-11`, `lysis-recycling`)
/// over `0..active_carrier0_channels` (`32` on single-carrier fields, `24` on
/// `V2-15` dual-carrier Carrier 0), zeroing any trailing channels.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] if `active_carrier0_channels > CHANNELS`,
/// `affinity` contains a nonfinite component, or symplectic rotation refuses
/// `affinity_norm_limit`.
pub fn corpse_affinity_direction(
    affinity: &[f64; CHANNELS],
    direction: CorpseDirection,
    active_carrier0_channels: usize,
    affinity_norm_limit: f64,
) -> Result<[f64; CHANNELS], CoreError> {
    if active_carrier0_channels > CHANNELS {
        return Err(CoreError::NumericalInvalid {
            quantity: "active carrier-0 channels",
            detail: format!("{active_carrier0_channels} exceeds {CHANNELS}"),
        });
    }
    match direction {
        CorpseDirection::GeneticAffinity => {
            if active_carrier0_channels == CHANNELS {
                return Ok(*affinity);
            }
            let mut out = [0.0_f64; CHANNELS];
            for (dst, &src) in out[..active_carrier0_channels]
                .iter_mut()
                .zip(&affinity[..active_carrier0_channels])
            {
                if !src.is_finite() {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "corpse affinity direction",
                        detail: format!("component {src} is not finite"),
                    });
                }
                *dst = src + 0.0;
            }
            Ok(out)
        }
        CorpseDirection::NegatedAffinity => {
            let mut out = [0.0_f64; CHANNELS];
            for (dst, &src) in out[..active_carrier0_channels]
                .iter_mut()
                .zip(&affinity[..active_carrier0_channels])
            {
                if !src.is_finite() {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "corpse affinity direction",
                        detail: format!("component {src} is not finite"),
                    });
                }
                *dst = if src == 0.0 { 0.0 } else { -src };
            }
            Ok(out)
        }
        CorpseDirection::SymplecticAffinity => {
            symplectic_rotate_channels(affinity, active_carrier0_channels, affinity_norm_limit)
        }
    }
}

/// Chemical direction `J a_eff` deposited by obligate symplectic metabolic waste
/// (`V2-11`, `symplectic-waste`) strictly into Carrier 0
/// (`0..active_carrier0_channels`, `32` single-carrier or `24` under `V2-15`),
/// guaranteeing `a_eff · J a_eff == 0` and exact left-fold
/// `‖J a_eff‖₂² <= affinity_norm_limit²`.
///
/// # Errors
/// As [`symplectic_rotate_channels`].
pub fn symplectic_waste_direction(
    affinity: &[f64; CHANNELS],
    active_carrier0_channels: usize,
    affinity_norm_limit: f64,
) -> Result<[f64; CHANNELS], CoreError> {
    symplectic_rotate_channels(affinity, active_carrier0_channels, affinity_norm_limit)
}
