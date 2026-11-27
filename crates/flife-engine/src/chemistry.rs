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

/// Paired four-packet exact channel-order (`0..32`) dot products for two
/// organisms simultaneously: `([a·x0..a·x3], [b·y0..b·y3])`.
///
/// Interleaving two organisms' 4-packet dot products exposes 4 independent
/// NEON `fadd.2d` dependency chains (saturating all 4 FP add pipelines on
/// Apple Silicon while overlapping both organisms' L2 composition loads) while
/// summing every single dot product strictly in channel order `0..32`
/// (0-ULP bit-identical to two separate [`dot4_channels`] calls).
#[inline(always)]
#[expect(
    clippy::too_many_arguments,
    reason = "private 2-organism × 4-packet exact channel-order dot product kernel"
)]
fn dot4_channels_pair(
    aff_a: &[f64; CHANNELS],
    a0: &[f64; CHANNELS],
    a1: &[f64; CHANNELS],
    a2: &[f64; CHANNELS],
    a3: &[f64; CHANNELS],
    aff_b: &[f64; CHANNELS],
    b0: &[f64; CHANNELS],
    b1: &[f64; CHANNELS],
    b2: &[f64; CHANNELS],
    b3: &[f64; CHANNELS],
) -> ([f64; 4], [f64; 4]) {
    let mut da = [0.0_f64; 4];
    let mut db = [0.0_f64; 4];
    for ((((&aa, &x0), &x1), (&x2, &x3)), (((&ab, &y0), &y1), (&y2, &y3))) in aff_a
        .as_chunks::<2>()
        .0
        .iter()
        .zip(a0.as_chunks::<2>().0)
        .zip(a1.as_chunks::<2>().0)
        .zip(a2.as_chunks::<2>().0.iter().zip(a3.as_chunks::<2>().0))
        .zip(
            aff_b
                .as_chunks::<2>()
                .0
                .iter()
                .zip(b0.as_chunks::<2>().0)
                .zip(b1.as_chunks::<2>().0)
                .zip(b2.as_chunks::<2>().0.iter().zip(b3.as_chunks::<2>().0)),
        )
    {
        for k in 0..2 {
            let a_k = aa[k];
            let b_k = ab[k];
            da = [
                da[0] + a_k * x0[k],
                da[1] + a_k * x1[k],
                da[2] + a_k * x2[k],
                da[3] + a_k * x3[k],
            ];
            db = [
                db[0] + b_k * y0[k],
                db[1] + b_k * y1[k],
                db[2] + b_k * y2[k],
                db[3] + b_k * y3[k],
            ];
        }
    }
    (da, db)
}

/// Four-packet exact channel-order (`0..32`) dot products `a · c_k` for
/// `k ∈ 0..4`. Chunking by 4 channels lets LLVM's SLP vectorizer emit pure
/// 2-wide NEON `fmul.2d` + `fadd.2d` across packets `(c0, c1)` and `(c2, c3)`
/// with zero scalar lane-extraction chains while preserving exact left-to-right
/// IEEE-754 summation per packet.
#[inline(always)]
fn dot4_channels(
    aff: &[f64; CHANNELS],
    c0: &[f64; CHANNELS],
    c1: &[f64; CHANNELS],
    c2: &[f64; CHANNELS],
    c3: &[f64; CHANNELS],
) -> [f64; 4] {
    let mut d = [0.0_f64; 4];
    for ((((&a, &x), &y), &z), &w) in aff
        .as_chunks::<4>()
        .0
        .iter()
        .zip(c0.as_chunks::<4>().0)
        .zip(c1.as_chunks::<4>().0)
        .zip(c2.as_chunks::<4>().0)
        .zip(c3.as_chunks::<4>().0)
    {
        for k in 0..4 {
            let ak = a[k];
            d = [
                d[0] + ak * x[k],
                d[1] + ak * y[k],
                d[2] + ak * z[k],
                d[3] + ak * w[k],
            ];
        }
    }
    d
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
            let [d0, d1, d2, d3] = dot4_channels(
                &affinity.0,
                &packets[0].composition,
                &packets[1].composition,
                &packets[2].composition,
                &packets[3].composition,
            );
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
        settle_efficiencies_trusted(self.toxicity_sensitivity, list, budget)
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
        if let &[p0, p1, p2, p3] = packets
            && let (Some(c0), Some(c1), Some(c2), Some(c3)) = (
                compositions.get(p0.1 as usize),
                compositions.get(p1.1 as usize),
                compositions.get(p2.1 as usize),
                compositions.get(p3.1 as usize),
            )
        {
            let [d0, d1, d2, d3] = dot4_channels(aff, c0, c1, c2, c3);
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
            return settle_4_efficiencies_trusted(
                self.toxicity_sensitivity,
                [p0.0, p1.0, p2.0, p3.0],
                [
                    max_eff * responses[0],
                    max_eff * responses[1],
                    max_eff * responses[2],
                    max_eff * responses[3],
                ],
                budget,
            );
        }
        self.settle_indexed_slice_general(aff, packets, compositions, budget)
    }

    #[cold]
    fn settle_indexed_slice_general(
        &self,
        aff: &[f64; CHANNELS],
        packets: &[(f64, u32)],
        compositions: &[[f64; CHANNELS]],
        budget: &mut EnergyBudget,
    ) -> Result<UptakeSettlement, CoreError> {
        let mut efficiencies = [(0.0, 0.0); 4];
        let mut scored = Vec::new();
        let list: &mut [(f64, f64)] = if packets.len() <= efficiencies.len() {
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
        settle_efficiencies_trusted(self.toxicity_sensitivity, list, budget)
    }

    /// Settle two 4-packet organisms simultaneously given already-resolved
    /// composition references, keeping the fused dot-product, `tanh_4`, and
    /// settlement loop out-of-line so callers stay compact with zero redundant
    /// bounds checks or stack spills, while preserving 0-ULP bit-exact identity
    /// with two sequential [`Chemistry::settle_indexed_slice`] calls.
    ///
    /// # Errors
    /// Returns the first error encountered in organism A before organism B,
    /// matching sequential evaluation.
    #[inline(never)]
    #[expect(
        clippy::too_many_arguments,
        reason = "paired 2-organism chemistry settlement taking both organisms' affinities, grants, compositions, and budgets"
    )]
    pub(crate) fn settle_pair_4_refs(
        &self,
        aff_a: &[f64; CHANNELS],
        grants_a: [f64; 4],
        comps_a: [&[f64; CHANNELS]; 4],
        budget_a: &mut EnergyBudget,
        aff_b: &[f64; CHANNELS],
        grants_b: [f64; 4],
        comps_b: [&[f64; CHANNELS]; 4],
        budget_b: &mut EnergyBudget,
    ) -> Result<(UptakeSettlement, UptakeSettlement), CoreError> {
        let (da, db) = dot4_channels_pair(
            aff_a, comps_a[0], comps_a[1], comps_a[2], comps_a[3], aff_b, comps_b[0], comps_b[1],
            comps_b[2], comps_b[3],
        );
        let args_a = [
            self.gain * da[0] - self.threshold,
            self.gain * da[1] - self.threshold,
            self.gain * da[2] - self.threshold,
            self.gain * da[3] - self.threshold,
        ];
        let resp_a = match tanh_4(args_a) {
            Some(r) => r,
            None => eval_tanh_4_fallback(args_a, da)?,
        };
        let max_eff = self.maximum_efficiency;
        let settle_a = settle_4_efficiencies_trusted(
            self.toxicity_sensitivity,
            grants_a,
            [
                max_eff * resp_a[0],
                max_eff * resp_a[1],
                max_eff * resp_a[2],
                max_eff * resp_a[3],
            ],
            budget_a,
        )?;

        let args_b = [
            self.gain * db[0] - self.threshold,
            self.gain * db[1] - self.threshold,
            self.gain * db[2] - self.threshold,
            self.gain * db[3] - self.threshold,
        ];
        let resp_b = match tanh_4(args_b) {
            Some(r) => r,
            None => eval_tanh_4_fallback(args_b, db)?,
        };
        let settle_b = settle_4_efficiencies_trusted(
            self.toxicity_sensitivity,
            grants_b,
            [
                max_eff * resp_b[0],
                max_eff * resp_b[1],
                max_eff * resp_b[2],
                max_eff * resp_b[3],
            ],
            budget_b,
        )?;

        Ok((settle_a, settle_b))
    }

    /// Settle two 4-packet organisms simultaneously against a deduplicated
    /// `compositions` table, interleaving their exact channel-order dot
    /// products via [`dot4_channels_pair`] to saturate all 4 FP add pipelines
    /// while preserving 0-ULP bit-exact identity with two sequential
    /// [`Chemistry::settle_indexed_slice`] calls.
    ///
    /// # Errors
    /// Returns the first error encountered in organism A before organism B,
    /// matching sequential evaluation.
    #[cfg(test)]
    #[inline]
    #[expect(
        clippy::too_many_arguments,
        reason = "paired 2-organism chemistry settlement taking both organisms' affinities, packets, and budgets"
    )]
    pub(crate) fn settle_indexed_pair_4(
        &self,
        aff_a: &[f64; CHANNELS],
        packets_a: [(f64, u32); 4],
        budget_a: &mut EnergyBudget,
        aff_b: &[f64; CHANNELS],
        packets_b: [(f64, u32); 4],
        budget_b: &mut EnergyBudget,
        compositions: &[[f64; CHANNELS]],
    ) -> Result<(UptakeSettlement, UptakeSettlement), CoreError> {
        let a0 = compositions
            .get(packets_a[0].1 as usize)
            .ok_or_else(|| err_comp_oob(packets_a[0].1, compositions.len()))?;
        let a1 = compositions
            .get(packets_a[1].1 as usize)
            .ok_or_else(|| err_comp_oob(packets_a[1].1, compositions.len()))?;
        let a2 = compositions
            .get(packets_a[2].1 as usize)
            .ok_or_else(|| err_comp_oob(packets_a[2].1, compositions.len()))?;
        let a3 = compositions
            .get(packets_a[3].1 as usize)
            .ok_or_else(|| err_comp_oob(packets_a[3].1, compositions.len()))?;
        let b0 = compositions
            .get(packets_b[0].1 as usize)
            .ok_or_else(|| err_comp_oob(packets_b[0].1, compositions.len()))?;
        let b1 = compositions
            .get(packets_b[1].1 as usize)
            .ok_or_else(|| err_comp_oob(packets_b[1].1, compositions.len()))?;
        let b2 = compositions
            .get(packets_b[2].1 as usize)
            .ok_or_else(|| err_comp_oob(packets_b[2].1, compositions.len()))?;
        let b3 = compositions
            .get(packets_b[3].1 as usize)
            .ok_or_else(|| err_comp_oob(packets_b[3].1, compositions.len()))?;

        self.settle_pair_4_refs(
            aff_a,
            [
                packets_a[0].0,
                packets_a[1].0,
                packets_a[2].0,
                packets_a[3].0,
            ],
            [a0, a1, a2, a3],
            budget_a,
            aff_b,
            [
                packets_b[0].0,
                packets_b[1].0,
                packets_b[2].0,
                packets_b[3].0,
            ],
            [b0, b1, b2, b3],
            budget_b,
        )
    }
}

#[cold]
#[inline(never)]
fn eval_tanh_4_fallback(args: [f64; 4], dots: [f64; 4]) -> Result<[f64; 4], CoreError> {
    Ok([
        tanh(args[0]).ok_or_else(|| err_nonfinite_dot(dots[0]))?,
        tanh(args[1]).ok_or_else(|| err_nonfinite_dot(dots[1]))?,
        tanh(args[2]).ok_or_else(|| err_nonfinite_dot(dots[2]))?,
        tanh(args[3]).ok_or_else(|| err_nonfinite_dot(dots[3]))?,
    ])
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
        let bound = limit * limit;
        let exceeds = if bound >= f64::MIN_POSITIVE {
            squared > bound
        } else if direction.iter().any(|&c| c.abs() > limit) {
            true
        } else {
            const UP_MID: f64 = f64::from_bits((1023_u64 + 530) << 52);
            const UP_TINY: f64 = f64::from_bits((1023_u64 + 1020) << 52);
            let up = if limit >= 1.0e-300 { UP_MID } else { UP_TINY };
            let s_limit = limit * up;
            let s_squared: f64 = direction
                .iter()
                .map(|&c| {
                    let sc = c * up;
                    sc * sc
                })
                .sum();
            s_squared > s_limit * s_limit
        };
        if exceeds {
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
    settle_efficiencies_trusted(toxicity_sensitivity, packets, budget)
}

#[cold]
#[inline(never)]
fn find_invalid_4_packet_err(grants: [f64; 4], efficiencies: [f64; 4]) -> CoreError {
    for k in 0..4 {
        let g = grants[k];
        if !(0.0..f64::INFINITY).contains(&g) {
            return err_invalid_grant(g);
        }
        let e = efficiencies[k];
        if !(-1.0..=1.0).contains(&e) {
            return err_invalid_efficiency(e);
        }
    }
    err_invalid_grant(f64::NAN)
}

/// Nonnegative Neumaier compensated step, bit-identical to
/// [`CompensatedSum::add`] whenever `sum >= 0.0` and `value >= 0.0` (so
/// `sum.abs() >= value.abs()` reduces to `sum >= value` without `fabs`).
#[inline(always)]
fn neumaier_add_nonneg(sum: &mut f64, comp: &mut f64, value: f64) {
    let s = *sum;
    let total = s + value;
    *comp += if s >= value {
        (s - total) + value
    } else {
        (value - total) + s
    };
    *sum = total;
}

#[inline]
fn settle_4_efficiencies_trusted(
    toxicity_sensitivity: f64,
    grants: [f64; 4],
    efficiencies: [f64; 4],
    budget: &mut EnergyBudget,
) -> Result<UptakeSettlement, CoreError> {
    let valid = (grants[0] >= 0.0)
        & (grants[0] < f64::INFINITY)
        & (grants[1] >= 0.0)
        & (grants[1] < f64::INFINITY)
        & (grants[2] >= 0.0)
        & (grants[2] < f64::INFINITY)
        & (grants[3] >= 0.0)
        & (grants[3] < f64::INFINITY)
        & (efficiencies[0].abs() <= 1.0)
        & (efficiencies[1].abs() <= 1.0)
        & (efficiencies[2].abs() <= 1.0)
        & (efficiencies[3].abs() <= 1.0);
    if !valid {
        return Err(find_invalid_4_packet_err(grants, efficiencies));
    }

    // For finite `x >= 0.0`, `CompensatedSum::ZERO.add(x)` produces
    // `sum = 0.0 + x` and `comp = 0.0` bit-for-bit across all values including
    // `-0.0`, `+0.0`, subnormals, and `f64::MAX`.
    let mut abs_sum = grants[0] + 0.0;
    let mut abs_comp = 0.0_f64;
    let mut gain_sum = (efficiencies[0].max(0.0) * grants[0]) + 0.0;
    let mut gain_comp = 0.0_f64;
    let mut harm_sum = ((-efficiencies[0]).max(0.0) * grants[0]) + 0.0;
    let mut harm_comp = 0.0_f64;

    for k in 1..4 {
        let granted = grants[k];
        let efficiency = efficiencies[k];
        neumaier_add_nonneg(&mut abs_sum, &mut abs_comp, granted);
        neumaier_add_nonneg(&mut gain_sum, &mut gain_comp, efficiency.max(0.0) * granted);
        neumaier_add_nonneg(
            &mut harm_sum,
            &mut harm_comp,
            (-efficiency).max(0.0) * granted,
        );
    }

    let gain = gain_sum + gain_comp;
    let requested_toxicity = toxicity_sensitivity * (harm_sum + harm_comp);
    if gain > 0.0 {
        budget.credit(gain)?;
    }
    let toxicity = budget.pay(requested_toxicity)?;
    Ok(UptakeSettlement {
        absorbed: abs_sum + abs_comp,
        gain,
        toxicity,
    })
}

#[inline]
fn settle_efficiencies_trusted(
    toxicity_sensitivity: f64,
    packets: &[(f64, f64)],
    budget: &mut EnergyBudget,
) -> Result<UptakeSettlement, CoreError> {
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
                for &src in affinity {
                    if !src.is_finite() {
                        return Err(CoreError::NumericalInvalid {
                            quantity: "corpse affinity direction",
                            detail: format!("component {src} is not finite"),
                        });
                    }
                }
                return Ok(affinity.map(|src| src + 0.0));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settle_indexed_pair_4_matches_scalar_settle_bit_for_bit() {
        let chem = Chemistry::new(0.85, 3.2, 0.25, 1.4, 1.0).expect("valid chemistry");
        let mut aff_a = [0.0_f64; CHANNELS];
        let mut aff_b = [0.0_f64; CHANNELS];
        let mut norm_a = 0.0_f64;
        let mut norm_b = 0.0_f64;
        for c in 0..CHANNELS {
            aff_a[c] = ((c as f64) * 0.37 - 5.1).sin();
            aff_b[c] = ((c as f64) * 0.71 + 1.3).cos();
            norm_a += aff_a[c] * aff_a[c];
            norm_b += aff_b[c] * aff_b[c];
        }
        let scale_a = 0.9 / norm_a.sqrt();
        let scale_b = 0.85 / norm_b.sqrt();
        for c in 0..CHANNELS {
            aff_a[c] *= scale_a;
            aff_b[c] *= scale_b;
        }
        let affinity_a = Affinity::new(aff_a, &chem).expect("aff_a");
        let affinity_b = Affinity::new(aff_b, &chem).expect("aff_b");

        let mut comps = [[0.0_f64; CHANNELS]; 6];
        for (idx, comp) in comps.iter_mut().enumerate() {
            if idx == 0 {
                continue; // neutral zero cell
            }
            for (c, slot) in comp.iter_mut().enumerate() {
                *slot = (((idx * 31 + c * 17) as f64) * 0.23).sin() * 0.15;
            }
        }

        let pkts_a = [(0.125, 1_u32), (0.0, 0_u32), (0.375, 2_u32), (0.5, 3_u32)];
        let pkts_b = [(0.4, 4_u32), (0.2, 5_u32), (0.1, 1_u32), (0.3, 0_u32)];
        let chem_pkts_a: Vec<ChemicalPacket> = pkts_a
            .iter()
            .map(|&(granted, idx)| ChemicalPacket {
                granted,
                composition: comps[idx as usize],
            })
            .collect();
        let chem_pkts_b: Vec<ChemicalPacket> = pkts_b
            .iter()
            .map(|&(granted, idx)| ChemicalPacket {
                granted,
                composition: comps[idx as usize],
            })
            .collect();

        // Check both unclipped and toxicity-clipped budgets.
        for &(start_a, start_b) in &[(10.0, 10.0), (0.01, 5.0), (5.0, 0.005)] {
            let mut ref_ba = EnergyBudget::new(start_a).expect("ba");
            let mut ref_bb = EnergyBudget::new(start_b).expect("bb");
            let ref_sa = chem
                .settle(&affinity_a, &chem_pkts_a, &mut ref_ba)
                .expect("ref_sa");
            let ref_sb = chem
                .settle(&affinity_b, &chem_pkts_b, &mut ref_bb)
                .expect("ref_sb");

            let mut pair_ba = EnergyBudget::new(start_a).expect("pba");
            let mut pair_bb = EnergyBudget::new(start_b).expect("pbb");
            let (pair_sa, pair_sb) = chem
                .settle_indexed_pair_4(
                    &aff_a,
                    pkts_a,
                    &mut pair_ba,
                    &aff_b,
                    pkts_b,
                    &mut pair_bb,
                    &comps,
                )
                .expect("pair");

            assert_eq!(pair_sa.absorbed.to_bits(), ref_sa.absorbed.to_bits());
            assert_eq!(pair_sa.gain.to_bits(), ref_sa.gain.to_bits());
            assert_eq!(
                pair_sa.toxicity.requested.to_bits(),
                ref_sa.toxicity.requested.to_bits()
            );
            assert_eq!(
                pair_sa.toxicity.paid.to_bits(),
                ref_sa.toxicity.paid.to_bits()
            );
            assert_eq!(pair_ba.remaining().to_bits(), ref_ba.remaining().to_bits());

            assert_eq!(pair_sb.absorbed.to_bits(), ref_sb.absorbed.to_bits());
            assert_eq!(pair_sb.gain.to_bits(), ref_sb.gain.to_bits());
            assert_eq!(
                pair_sb.toxicity.requested.to_bits(),
                ref_sb.toxicity.requested.to_bits()
            );
            assert_eq!(
                pair_sb.toxicity.paid.to_bits(),
                ref_sb.toxicity.paid.to_bits()
            );
            assert_eq!(pair_bb.remaining().to_bits(), ref_bb.remaining().to_bits());
        }

        // Verify out-of-bounds index refusal reports organism A before organism B.
        let mut ba = EnergyBudget::new(1.0).expect("ba");
        let mut bb = EnergyBudget::new(1.0).expect("bb");
        let bad_a = [(0.1, 99_u32), (0.1, 0), (0.1, 0), (0.1, 0)];
        let bad_b = [(0.1, 88_u32), (0.1, 0), (0.1, 0), (0.1, 0)];
        let err = chem
            .settle_indexed_pair_4(&aff_a, bad_a, &mut ba, &aff_b, bad_b, &mut bb, &comps)
            .expect_err("oob must fail");
        assert!(format!("{err}").contains("99"));
    }

    #[test]
    fn settle_4_efficiencies_trusted_matches_scalar_edge_cases_bit_for_bit() {
        let edge_grants = [
            0.0,
            -0.0,
            f64::from_bits(1), // smallest subnormal
            f64::MIN_POSITIVE,
            1.0e-12,
            0.125,
            1.0,
            1.0e6,
            1.0e150,
            f64::MAX * 0.25,
        ];
        let edge_effs = [
            -1.0,
            -0.75,
            -f64::from_bits(1),
            -0.0,
            0.0,
            f64::from_bits(1),
            0.5,
            1.0,
        ];
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next_u64 = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        // 2,000 adversarial + pseudorandom 4-packet cases across kappa and budgets.
        for case in 0..2_000 {
            let (grants, effs) = if case < 400 {
                let g =
                    std::array::from_fn(|_| edge_grants[(next_u64() as usize) % edge_grants.len()]);
                let e = std::array::from_fn(|_| edge_effs[(next_u64() as usize) % edge_effs.len()]);
                (g, e)
            } else {
                let g =
                    std::array::from_fn(|_| ((next_u64() >> 11) as f64) * 2.0_f64.powi(-53) * 10.0);
                let e =
                    std::array::from_fn(|_| ((next_u64() >> 11) as f64) * 2.0_f64.powi(-52) - 1.0);
                (g, e)
            };
            let pairs = [
                (grants[0], effs[0]),
                (grants[1], effs[1]),
                (grants[2], effs[2]),
                (grants[3], effs[3]),
            ];
            let kappa = if case % 3 == 0 { 0.2 } else { 1.75 };
            let start = if case % 5 == 0 { 0.001 } else { 5.0 };
            let mut b_ref = EnergyBudget::new(start).expect("b_ref");
            let mut b_fast = EnergyBudget::new(start).expect("b_fast");
            let r_ref = settle_efficiencies(kappa, &pairs, &mut b_ref);
            let r_fast = settle_4_efficiencies_trusted(kappa, grants, effs, &mut b_fast);
            match (r_ref, r_fast) {
                (Ok(a), Ok(b)) => {
                    assert_eq!(
                        a.absorbed.to_bits(),
                        b.absorbed.to_bits(),
                        "case {case} absorbed"
                    );
                    assert_eq!(a.gain.to_bits(), b.gain.to_bits(), "case {case} gain");
                    assert_eq!(
                        a.toxicity.requested.to_bits(),
                        b.toxicity.requested.to_bits(),
                        "case {case} tox.req"
                    );
                    assert_eq!(
                        a.toxicity.paid.to_bits(),
                        b.toxicity.paid.to_bits(),
                        "case {case} tox.paid"
                    );
                    assert_eq!(
                        b_ref.remaining().to_bits(),
                        b_fast.remaining().to_bits(),
                        "case {case} remaining"
                    );
                    assert_eq!(
                        b_ref.is_exhausted(),
                        b_fast.is_exhausted(),
                        "case {case} exh"
                    );
                }
                (Err(ea), Err(eb)) => assert_eq!(format!("{ea}"), format!("{eb}"), "case {case}"),
                (a, b) => panic!("case {case} mismatch: ref={a:?}, fast={b:?}"),
            }
        }

        // Exact error ordering on invalid grants and efficiencies (grant before efficiency, index 0..4).
        let bad_cases = [
            ([-1e-300, 1.0, 1.0, 1.0], [0.0; 4]),
            ([f64::NAN, 1.0, 1.0, 1.0], [2.0; 4]),
            ([1.0, f64::INFINITY, 1.0, 1.0], [0.0, 2.0, 0.0, 0.0]),
            ([1.0, 1.0, f64::NEG_INFINITY, 1.0], [0.0; 4]),
            ([1.0, 1.0, 1.0, -0.5], [0.0; 4]),
            ([1.0; 4], [1.0 + f64::EPSILON, 0.0, 0.0, 0.0]),
            ([1.0; 4], [0.0, -1.0 - f64::EPSILON, 0.0, 0.0]),
            ([1.0; 4], [0.0, 0.0, f64::NAN, 0.0]),
            ([1.0; 4], [0.0, 0.0, 0.0, f64::INFINITY]),
        ];
        for (grants, effs) in bad_cases {
            let pairs = [
                (grants[0], effs[0]),
                (grants[1], effs[1]),
                (grants[2], effs[2]),
                (grants[3], effs[3]),
            ];
            let mut b_ref = EnergyBudget::new(1.0).expect("b_ref");
            let mut b_fast = EnergyBudget::new(1.0).expect("b_fast");
            let e_ref = settle_efficiencies(0.2, &pairs, &mut b_ref).expect_err("ref err");
            let e_fast = settle_4_efficiencies_trusted(0.2, grants, effs, &mut b_fast)
                .expect_err("fast err");
            assert_eq!(format!("{e_ref}"), format!("{e_fast}"));
        }
    }
}
