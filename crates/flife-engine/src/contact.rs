//! Soft-disk contact and like-to-like adhesion for the
//! `field-ecology-v1-contact` models (plan §5.4's "direct collisions,
//! adhesion … define later model variants").
//!
//! Under a contact model every organism is a disk of diameter `D`
//! (`[contact] diameter`). After phase 8, each organism that survives the
//! tick is displaced by every other survivor within reach, all computed from
//! the settled positions before any displacement:
//!
//! ```text
//! Δ_i = Σ_{j ≠ i} p_ij u_ij,
//! p_ij = (k/2) (D − d)                                  for d < D,
//! p_ij = −(k_a/2) w_ij (d − D)(R D − d) / ((R − 1) D)   for D ≤ d < R D,
//! w_ij = max(0, cos(a_i, a_j))²,
//! ```
//!
//! where `d = d_ij` is the minimum-image distance, `u_ij` the unit vector from
//! `j` to `i`, `k` the stiffness, `k_a` the adhesion, `R` the adhesion range,
//! and `a_i` organism `i`'s affinity direction. Overlapping bodies push apart;
//! bodies near touching pull together in proportion to how alike their
//! chemistry is, a pull that vanishes at touching and at reach. Two organisms
//! at one point separate along `±x`, the higher identity towards `+x`.
//! `Δ_i` is clamped radially to `min(D/2, ½ min(h_x, h_y))`, and the position
//! wrapped.
//!
//! The `field-ecology-v1-contact-sorting` models also push unlike chemistry
//! apart (cell sorting). Within reach the weight becomes
//!
//! ```text
//! w_ij = (1 + r) max(0, cos(a_i, a_j))² − r,
//! ```
//!
//! with `r` the `[contact] repulsion`: pairs with `max(0, cos)² > r / (1 + r)`
//! pull, less than under the plain rule, and less alike pairs push apart over
//! the same shell, by up to `r` times the pull of identical chemistry. At
//! `r = 0` the rule is the plain one. Repulsion acts only where adhesion does
//! (`k_a > 0`, `R > 1`).
//!
//! Contact moves positions only. It changes no velocity and no energy, so the
//! ledger is the same with or without it. Emissions and births of the tick
//! use the settled positions, before contact.
//!
//! The `field-ecology-v1-contact-inhibition` models relax `S` times per tick
//! (`[contact] sweeps`). Sweep 1 is the rule above; each later sweep pushes
//! overlapping bodies apart only, `p_ij = (k/2)(D − d)` for `d < D` (clamped
//! as above), from the positions `wrap(x_i + Σ_{t<s} Δ_i^t)`, and the
//! survivor ends at `wrap(x_i + Σ_s Δ_i^s)`. Adhesion thus pulls once per tick
//! and the sweeps resolve the overlap it and motion leave: summed over every
//! neighbour each sweep, the pull would outweigh the soft core
//! (`∫ V d²r < 0` for `k_a ≳ 1.3 k`) and bodies would stack. They also
//! inhibit locomotion. With `P_i = Σ_s Σ_{j: d < D} p_ij u_ij`, the push alone
//! of the bodies a survivor overlapped, its velocity loses the component
//! against `P_i`:
//!
//! ```text
//! n = P_i / |P_i|,   v_i ← v_i − c min(0, v_i · n) n,
//! ```
//!
//! with `c` the `[contact] inhibition`, so a body slows its crawl into its
//! neighbours and keeps the rest of its velocity. Its speed never grows, so
//! motion's energy is unchanged here.
//!
//! Each sum runs over the neighbour bins, at least the reach wide, in a fixed
//! order, `(dy, dx)` from `(−1, −1)` to `(1, 1)`, and within a bin in identity
//! order. Every body's sum is its own, so the bodies are split among the
//! workers and a world's contact displacement is the same at any worker
//! count.

use flife_core::ledger::CompensatedSum;
use flife_core::model::{
    FIELD_ECOLOGY_V1_CONTACT_INHIBITION, FIELD_ECOLOGY_V1_CONTACT_INHIBITION_CHEMISTRY_ONLY,
    FIELD_ECOLOGY_V1_CONTACT_INHIBITION_FRESH_OFFSPRING, MAXIMUM_CONTACT_SWEEPS, is_v2_model,
};
use flife_core::numerics::{normalize_turns, sin_cos_turns};
use flife_core::{CoreError, ValidatedConfig};
use flife_field::FieldGrid;
use flife_genome::{AFFINITY_TERMS, CADHERIN_TERMS};

/// Maximum per-tick angular polarity rotation in radians (`V2-08`, `C003`).
pub const MAX_POLARITY_DTHETA_RAD: f64 = 0.25;

/// Exact binary energy quantum `Q_tr = 2^-30` for passive kin trophallaxis
/// (`V2-13`, `C013`). Because `2^-30` is an exact power of two, converting
/// signed `i64` quantum totals to `f64` (`k as f64 * TROPHALLAXIS_QUANTUM`) is
/// bit-exact for every `|k| <= 2^53`, guaranteeing 0-bit integer ledger
/// conservation (`INV-03`) and bit-exact worker-count determinism (`INV-11`).
pub const TROPHALLAXIS_QUANTUM: f64 = 1.0 / ((1_u64 << 30) as f64);

/// `V2-08` (`pcp-polarity`) Sweep-1 physics parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PcpSweepParams {
    /// Timestep `Δt` in simulation seconds.
    pub dt: f64,
    /// Angular torque mobility `μ_θ >= 0` (`0.0` freezes polarity vectors).
    pub torque_mobility: f64,
    /// Basal-apical energy-gradient cue strength `k_cue >= 0`.
    pub gradient_cue_strength: f64,
}

/// `V2-13` (`trophallaxis`, `C013`) passive exact-quantum Sweep-1 parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrophallaxisSweepParams {
    /// Timestep `Δt` in simulation seconds.
    pub dt: f64,
    /// Gap-junction conductance `G_gap >= 0` in `s^-1`.
    pub conductance_per_second: f64,
    /// Transfer efficiency `ρ_tr ∈ (0, 0.99]`.
    pub transfer_efficiency: f64,
    /// Donor energy reserve floor `E_reserve` (`> 0`).
    pub reserve_energy: f64,
    /// Maximum fraction `f_max ∈ (0, 1]` of `(E_i - E_reserve).max(0)` exported per tick.
    pub maximum_export_fraction: f64,
    /// Exponent `p` applied to `max(0, cos(a_i, a_j))` (default `4`).
    pub kin_cosine_exponent: u32,
    /// Minimum genetic affinity cosine required for transfer (`∈ [0, 1]`, default `0.5`).
    pub kin_threshold: f64,
}

/// Exact integer quantum totals produced by one tick's passive kin trophallaxis
/// (`V2-13`, `C013`), satisfying `total_out_quanta == total_in_quanta + total_tax_quanta`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrophallaxisSummary {
    /// Total quanta debited from donors across the population.
    pub total_out_quanta: u64,
    /// Total quanta credited to recipients across the population.
    pub total_in_quanta: u64,
    /// Total quanta dissipated as `Flow::TrophallaxisLoss`.
    pub total_tax_quanta: u64,
}

impl TrophallaxisSummary {
    /// Total energy debited from donors (`total_out_quanta * 2^-30`).
    #[must_use]
    pub fn debited(self) -> f64 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "exact power-of-two binary quantum scaling for ecological energy totals"
        )]
        let value = (self.total_out_quanta as f64) * TROPHALLAXIS_QUANTUM;
        value
    }

    /// Total energy credited to recipients (`total_in_quanta * 2^-30`).
    #[must_use]
    pub fn credited(self) -> f64 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "exact power-of-two binary quantum scaling for ecological energy totals"
        )]
        let value = (self.total_in_quanta as f64) * TROPHALLAXIS_QUANTUM;
        value
    }

    /// Total energy dissipated as trophallaxis tax (`total_tax_quanta * 2^-30`).
    #[must_use]
    pub fn tax(self) -> f64 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "exact power-of-two binary quantum scaling for ecological energy totals"
        )]
        let value = (self.total_tax_quanta as f64) * TROPHALLAXIS_QUANTUM;
        value
    }
}

/// `V2-13` (`velocity-alignment`, `C019`) dissipative Sweep-1 parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VelocityAlignmentSweepParams {
    /// Alignment blend weight `μ_align ∈ [0, 0.4]`.
    pub strength: f64,
}

/// Optional `field-ecology-v2` Sweep-1 context (`V2-08` PCP polarity + EMT
/// adhesion gate, `V2-12` Notch-Delta neighbor averaging, `V2-13` cadherin-4d,
/// passive exact-quantum trophallaxis, and dissipative velocity alignment).
///
/// When all optional fields are `None`, Sweep 1 dispatches directly to the
/// baseline isotropic [`relax`] kernel bit-for-bit.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sweep1Context<'a> {
    /// Active `V2-08` PCP polarity parameters, if `pcp-polarity` is enabled.
    pub pcp: Option<PcpSweepParams>,
    /// Committed unit apical-basal polarity vectors `p_i ∈ S^1` (length `N`).
    pub polarities: Option<&'a [[f64; 2]]>,
    /// Heritable lateral-belt polarity strengths `α_pol,i ∈ [0, 1]` (length `N`).
    pub polarity_strengths: Option<&'a [f64]>,
    /// Heritable spontaneous apical wedge curvatures `β_i ∈ [-β_max, β_max]` (length `N`).
    pub wedge_curvatures: Option<&'a [f64]>,
    /// Observed energy gradients `∇q_i` (`gradients[0]`, length `N`).
    pub energy_gradients: Option<&'a [[f64; 2]]>,
    /// Combined EMT adhesion multipliers `emt_eff_i` (from `V2-08` `2 σ(o_adh)`
    /// and/or `V2-12` `m(h_1) = clamp(1 + h_1, 0, 1)`, length `N`), if active.
    pub emt_factors: Option<&'a [f64]>,
    /// Post-Phase-4 primary recurrent cell states `h_0 ∈ [-1, 1]` (length `N`),
    /// if `V2-12` (`notch-delta`) is enabled.
    pub cell_states_h0: Option<&'a [f32]>,
    /// Decoupled 4-D unit cadherin tags `c_i` (`V2-13` `cadherin-4d`, length `N`).
    pub cadherin_tags: Option<&'a [[f64; CADHERIN_TERMS]]>,
    /// Heritable cadherin cohesiveness scalars `s_i ∈ [0, 1]` (`V2-13`, length `N`).
    pub cadherin_cohesiveness: Option<&'a [f64]>,
    /// Active `V2-13` (`trophallaxis`) parameters, if enabled.
    pub trophallaxis: Option<TrophallaxisSweepParams>,
    /// Pre-trophallaxis survivor budgets `E_i` (`V2-13` `trophallaxis`, length `N`).
    pub energies: Option<&'a [f64]>,
    /// Active `V2-13` (`velocity-alignment`) parameters, if enabled.
    pub velocity_alignment: Option<VelocityAlignmentSweepParams>,
    /// Committed post-motion velocities `v_i` (`V2-13` `velocity-alignment`, length `N`).
    pub velocities: Option<&'a [[f64; 2]]>,
}

/// Mutable Sweep-1 outputs written by [`relax_sweep1_with_context`].
#[derive(Debug)]
pub struct Sweep1Output<'a> {
    /// Clamped per-body contact displacements (length `N`).
    pub displacements: &'a mut Vec<[f64; 2]>,
    /// Updated unit polarity vectors `p'_i ∈ S^1` when `pcp-polarity` is active
    /// (length `N`); `None` when disabled.
    pub next_polarities: Option<&'a mut Vec<[f64; 2]>>,
    /// Kin-weighted mean neighbor cell state `h̄_i ∈ [-1, 1]` when `V2-12`
    /// (`notch-delta`) is active (length `N`); `None` when disabled.
    pub neighbor_h0: Option<&'a mut Vec<f32>>,
    /// Signed per-body exact binary-quantum trophallaxis deltas `Δk_i` in units
    /// of [`TROPHALLAXIS_QUANTUM`] (`V2-13`, length `N`); `None` when disabled.
    pub trophallaxis_delta_quanta: Option<&'a mut Vec<i64>>,
    /// Population-wide exact integer quantum totals (`V2-13`); `None` when
    /// disabled.
    pub trophallaxis_summary: Option<&'a mut TrophallaxisSummary>,
    /// Updated speed-non-increasing aligned velocities `v'_i` (`V2-13`, length
    /// `N`); `None` when disabled.
    pub next_velocities: Option<&'a mut Vec<[f64; 2]>>,
}

/// Deterministic integer power `base^exp` in F64 via binary exponentiation
/// (avoids host libm `powi` variance).
#[must_use]
fn pow_u32(mut base: f64, mut exp: u32) -> f64 {
    let mut acc = 1.0_f64;
    while exp > 0 {
        if exp & 1 == 1 {
            acc *= base;
        }
        exp >>= 1;
        if exp > 0 {
            base *= base;
        }
    }
    acc
}

/// Pure `V2-13` (`cadherin-4d`, `C023`) decoupled pair adhesion weight
/// `min(s_i, s_j) * max(0, cos(c_i, c_j))^2` (`∈ [0, 1]`), strictly symmetric
/// under `i <-> j`.
#[must_use]
pub fn cadherin_pair_weight(
    c_i: &[f64; CADHERIN_TERMS],
    s_i: f64,
    c_j: &[f64; CADHERIN_TERMS],
    s_j: f64,
) -> f64 {
    let dot = c_i[0] * c_j[0] + c_i[1] * c_j[1] + c_i[2] * c_j[2] + c_i[3] * c_j[3];
    let alike = dot.clamp(0.0, 1.0);
    s_i.min(s_j) * (alike * alike)
}

/// Cadherin adhesion weight with optional cell-sorting repulsion `r >= 0`.
/// At `r == 0.0` this equals [`cadherin_pair_weight`] identically; at `s_i == 0`
/// or `s_j == 0` non-cohesive bodies exert neither pull nor sorting repulsion.
#[must_use]
fn cadherin_sorting_weight(
    c_i: &[f64; CADHERIN_TERMS],
    s_i: f64,
    c_j: &[f64; CADHERIN_TERMS],
    s_j: f64,
    repulsion: f64,
) -> f64 {
    let dot = c_i[0] * c_j[0] + c_i[1] * c_j[1] + c_i[2] * c_j[2] + c_i[3] * c_j[3];
    let alike = dot.clamp(0.0, 1.0);
    let alike_sq = alike * alike;
    let cohesion = s_i.min(s_j);
    if repulsion > 0.0 {
        cohesion * ((1.0 + repulsion) * alike_sq - repulsion)
    } else {
        cohesion * alike_sq
    }
}

/// Pure `V2-13` (`trophallaxis`, `C013`) raw directional energy demand
/// `Δt · G_gap · alike^p · (1 − d / (R D)) · (E_donor − E_recipient)` for a
/// directed pair with `donor_e > recipient_e` and genetic affinity cosine
/// `alike = max(0, cos(a_i, a_j))`.
#[must_use]
pub fn pair_trophallaxis_raw(
    donor_e: f64,
    recipient_e: f64,
    dist: f64,
    reach_distance: f64,
    alike: f64,
    cfg: &TrophallaxisSweepParams,
) -> f64 {
    if donor_e <= recipient_e
        || dist >= reach_distance
        || reach_distance <= 0.0
        || alike < cfg.kin_threshold
        || cfg.conductance_per_second <= 0.0
        || cfg.dt <= 0.0
    {
        return 0.0;
    }
    let spatial = (1.0 - dist / reach_distance).max(0.0);
    if spatial <= 0.0 {
        return 0.0;
    }
    let kin = pow_u32(alike, cfg.kin_cosine_exponent);
    let raw = cfg.dt * cfg.conductance_per_second * kin * spatial * (donor_e - recipient_e);
    if raw.is_finite() && raw > 0.0 {
        raw
    } else {
        0.0
    }
}

/// Canonical symmetric binary-quantum trophallaxis kernel (`V2-13`, `C013`).
///
/// Called identically on both the donor and recipient sides with the same
/// `(donor_e, donor_scale, recipient_e, dist, reach_distance, alike, cfg)`
/// (where `dist = dx.abs().hypot(dy.abs())` and `alike = max(0, cos(a_i, a_j))`
/// from genetic `traits.affinity()`). Returns `(q_out, q_in, q_tax)` in units
/// of [`TROPHALLAXIS_QUANTUM`] (`2^-30`) satisfying `q_out == q_in + q_tax` and
/// `0 <= q_in <= q_out` to zero bits.
#[must_use]
pub fn pair_trophallaxis_quanta(
    donor_e: f64,
    donor_scale: f64,
    recipient_e: f64,
    dist: f64,
    reach_distance: f64,
    alike: f64,
    cfg: &TrophallaxisSweepParams,
) -> (i64, i64, i64) {
    if donor_scale <= 0.0 {
        return (0, 0, 0);
    }
    let raw = pair_trophallaxis_raw(donor_e, recipient_e, dist, reach_distance, alike, cfg);
    if raw <= 0.0 {
        return (0, 0, 0);
    }
    let scaled = donor_scale * raw;
    let quanta_f = (scaled / TROPHALLAXIS_QUANTUM).floor();
    if !quanta_f.is_finite() || quanta_f < 1.0 {
        return (0, 0, 0);
    }
    const MAX_QUANTA: f64 = (1_i64 << 52) as f64;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "quanta_f is a finite nonnegative integer clamped to [1, 2^52]"
    )]
    let q_out = quanta_f.min(MAX_QUANTA) as i64;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "q_out <= 2^52 is exact in f64 and transfer_efficiency <= 0.99 keeps q_in <= q_out"
    )]
    let q_in = ((cfg.transfer_efficiency * (q_out as f64)).floor() as i64).clamp(0, q_out);
    let q_tax = q_out - q_in;
    (q_out, q_in, q_tax)
}

/// Pure `V2-13` (`velocity-alignment`, `C019`) dissipative kin velocity blend.
///
/// Computes `v_blend = (1 - μ) v_i + μ (sum_v / sum_w)` when `sum_w > 0` and
/// `μ > 0`, and clamps `v'_i` radially to `‖v_i‖₂` using `motion.rs`'s 1-ulp
/// shrink loop so `v'_i · v'_i <= v_i · v_i` strictly holds in F64 (`TK-04`:
/// zero unpaid kinetic work).
#[must_use]
pub fn align_velocity_dissipative(
    v_i: [f64; 2],
    sum_v: [f64; 2],
    sum_w: f64,
    strength: f64,
) -> [f64; 2] {
    let bound_sq = v_i[0] * v_i[0] + v_i[1] * v_i[1];
    if strength <= 0.0 || sum_w <= 0.0 || !bound_sq.is_finite() || bound_sq <= 0.0 {
        return v_i;
    }
    let mean_x = sum_v[0] / sum_w;
    let mean_y = sum_v[1] / sum_w;
    let keep = 1.0 - strength;
    let blend = [
        keep * v_i[0] + strength * mean_x,
        keep * v_i[1] + strength * mean_y,
    ];
    let blend_sq = blend[0] * blend[0] + blend[1] * blend[1];
    if !blend_sq.is_finite() || blend_sq <= 0.0 {
        return [0.0, 0.0];
    }
    if blend_sq <= bound_sq {
        return blend;
    }
    let scale = bound_sq.sqrt() / blend_sq.sqrt();
    let mut clamped = [blend[0] * scale, blend[1] * scale];
    let shrink = 1.0 - f64::EPSILON / 2.0;
    while clamped[0] * clamped[0] + clamped[1] * clamped[1] > bound_sq {
        let next = [clamped[0] * shrink, clamped[1] * shrink];
        if next == clamped {
            clamped = [
                if clamped[0] > 0.0 {
                    clamped[0].next_down()
                } else if clamped[0] < 0.0 {
                    clamped[0].next_up()
                } else {
                    0.0
                },
                if clamped[1] > 0.0 {
                    clamped[1].next_down()
                } else if clamped[1] < 0.0 {
                    clamped[1].next_up()
                } else {
                    0.0
                },
            ];
        } else {
            clamped = next;
        }
    }
    clamped
}

/// Pure `V2-08` PCP lateral + wedge adhesion modulation factor
/// `(1 - α_ij) + α_ij * G_apical * S_raw` (`∈ [0, 1]`), strictly symmetric
/// under `(i, j, r_hat) <-> (j, i, -r_hat)` with exact `0.0` apical-collapse
/// guard `G_apical = clamp(2 * (p_i · p_j), 0, 1)` when `p_i · p_j <= 0`.
#[must_use]
pub fn pcp_lateral_wedge_weight(
    p_i: [f64; 2],
    p_j: [f64; 2],
    r_hat: [f64; 2],
    alpha_ij: f64,
    beta_bar: f64,
) -> f64 {
    let pi_dot_pj = p_i[0] * p_j[0] + p_i[1] * p_j[1];
    let g_apical = (2.0 * pi_dot_pj).clamp(0.0, 1.0);
    let pi_dot_r = p_i[0] * r_hat[0] + p_i[1] * r_hat[1];
    let pj_dot_r = p_j[0] * r_hat[0] + p_j[1] * r_hat[1];
    let s_raw = ((pi_dot_pj - pi_dot_r * pj_dot_r + beta_bar * (pi_dot_r - pj_dot_r))
        / (1.0 + beta_bar * beta_bar))
        .clamp(0.0, 1.0);
    (1.0 - alpha_ij) + alpha_ij * g_apical * s_raw
}

/// Pure `V2-08` pair polarity torque kernel for body `i` from neighbor `j`
/// along unit vector `r_hat = (x_j - x_i) / d`:
/// `(p_i^⊥ · p_j) - (p_i^⊥ · r_hat)(p_j · r_hat) + β_bar (p_i^⊥ · r_hat)`.
#[must_use]
pub fn pcp_pair_torque_term(p_i: [f64; 2], p_j: [f64; 2], r_hat: [f64; 2], beta_bar: f64) -> f64 {
    let p_i_perp = [-p_i[1], p_i[0]];
    let perp_dot_pj = p_i_perp[0] * p_j[0] + p_i_perp[1] * p_j[1];
    let perp_dot_r = p_i_perp[0] * r_hat[0] + p_i_perp[1] * r_hat[1];
    let pj_dot_r = p_j[0] * r_hat[0] + p_j[1] * r_hat[1];
    perp_dot_pj - perp_dot_r * pj_dot_r + beta_bar * perp_dot_r
}

/// Rotate unit polarity vector `p` by `dtheta` radians (clamped to
/// `[-MAX_POLARITY_DTHETA_RAD, MAX_POLARITY_DTHETA_RAD]`) via host-independent
/// [`sin_cos_turns`] + [`normalize_turns`], then renormalize to unit length.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] if `p` or `dtheta` is nonfinite or `p` has
/// zero norm.
pub fn rotate_unit_polarity(p: [f64; 2], dtheta: f64) -> Result<[f64; 2], CoreError> {
    if !(p[0].is_finite() && p[1].is_finite() && dtheta.is_finite()) {
        return Err(CoreError::NumericalInvalid {
            quantity: "polarity rotation",
            detail: format!("nonfinite polarity {p:?} or angle {dtheta:?}"),
        });
    }
    let clamped = dtheta.clamp(-MAX_POLARITY_DTHETA_RAD, MAX_POLARITY_DTHETA_RAD);
    if clamped == 0.0 {
        let norm = p[0].hypot(p[1]);
        if !norm.is_finite() || norm <= 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "polarity vector",
                detail: format!("{p:?} has zero norm"),
            });
        }
        return Ok(p);
    }
    let turns = normalize_turns(clamped / core::f64::consts::TAU).ok_or_else(|| {
        CoreError::NumericalInvalid {
            quantity: "polarity rotation turns",
            detail: format!("angle {clamped:?} cannot normalize to turns"),
        }
    })?;
    let (sin_d, cos_d) = sin_cos_turns(turns).ok_or_else(|| CoreError::NumericalInvalid {
        quantity: "polarity rotation",
        detail: format!("turns {turns:?} left [0, 1)"),
    })?;
    let rx = cos_d * p[0] - sin_d * p[1];
    let ry = sin_d * p[0] + cos_d * p[1];
    let norm = rx.hypot(ry);
    if !norm.is_finite() || norm <= 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "rotated polarity",
            detail: format!("rotated polarity [{rx:?}, {ry:?}] has nonpositive norm"),
        });
    }
    Ok([rx / norm, ry / norm])
}

/// The most bins a side may have; a smaller reach uses coarser bins, which
/// find the same neighbours.
const MAXIMUM_BINS_PER_SIDE: usize = 2048;

/// The fewest bodies a worker sums; smaller worlds use fewer workers.
const MINIMUM_BODIES_PER_WORKER: usize = 4096;

/// A contact model's `[contact]` table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContactRules {
    /// `D`, the body diameter in world units.
    pub diameter: f64,
    /// `k`, the fraction of an overlap resolved per tick.
    pub stiffness: f64,
    /// `k_a`, the like-to-like pull.
    pub adhesion: f64,
    /// `R`, adhesion's reach in diameters.
    pub range: f64,
    /// `S`, relaxation sweeps per tick: 1 under the plain contact models.
    pub sweeps: u32,
    /// Whether survivors' pushes are kept for inhibition (the
    /// contact-inhibition models).
    pub inhibits: bool,
    /// `c`, the share of a survivor's velocity against its push removed: 0
    /// under the plain contact models.
    pub inhibition: f64,
    /// `r`, how strongly unlike chemistry pushes apart within reach, as a
    /// share of adhesion's pull: 0 except under the contact-sorting models.
    pub repulsion: f64,
}

impl ContactRules {
    /// The configuration's `[contact]` table, if it has one.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a value outside its bound (which
    /// validation already refuses).
    pub fn from_config(config: &ValidatedConfig) -> Result<Option<Self>, CoreError> {
        let Some(contact) = &config.config().contact else {
            return Ok(None);
        };
        let model = config.config().model.as_str();
        let inhibits = if is_v2_model(model) {
            contact.inhibition.is_some()
        } else {
            [
                FIELD_ECOLOGY_V1_CONTACT_INHIBITION,
                FIELD_ECOLOGY_V1_CONTACT_INHIBITION_CHEMISTRY_ONLY,
                FIELD_ECOLOGY_V1_CONTACT_INHIBITION_FRESH_OFFSPRING,
            ]
            .contains(&model)
        };
        let rules = Self {
            diameter: contact.diameter,
            stiffness: contact.stiffness,
            adhesion: contact.adhesion,
            range: contact.adhesion_range,
            sweeps: contact.sweeps.unwrap_or(1),
            inhibits,
            inhibition: contact.inhibition.unwrap_or(0.0),
            repulsion: contact.repulsion.unwrap_or(0.0),
        };
        rules.check()?;
        Ok(Some(rules))
    }

    fn check(&self) -> Result<(), CoreError> {
        let ok = self.diameter.is_finite()
            && self.diameter > 0.0
            && self.stiffness > 0.0
            && self.stiffness <= 1.0
            && (0.0..=1.0).contains(&self.adhesion)
            && self.range.is_finite()
            && self.range >= 1.0
            && (1..=MAXIMUM_CONTACT_SWEEPS).contains(&self.sweeps)
            && (0.0..=1.0).contains(&self.inhibition)
            && (0.0..=1.0).contains(&self.repulsion);
        if ok {
            Ok(())
        } else {
            Err(CoreError::NumericalInvalid {
                quantity: "contact",
                detail: format!(
                    "{self:?}: D > 0, k in (0, 1], k_a in [0, 1], R >= 1, S in [1, {MAXIMUM_CONTACT_SWEEPS}], c in [0, 1] and r in [0, 1]"
                ),
            })
        }
    }

    /// Whether bodies adhere at all.
    #[must_use]
    pub fn adheres(&self) -> bool {
        self.adhesion > 0.0 && self.range > 1.0
    }

    /// How far a body's neighbours can act on it.
    #[must_use]
    pub fn reach(&self) -> f64 {
        if self.adheres() {
            self.diameter * self.range
        } else {
            self.diameter
        }
    }
}

/// An organism's body radius in world units under `config`: half the contact
/// diameter for a contact model, none for any other, whose organisms are
/// points.
#[must_use]
pub fn body_radius(config: &ValidatedConfig) -> Option<f64> {
    config
        .config()
        .contact
        .as_ref()
        .map(|contact| 0.5 * contact.diameter)
}

/// One cached in-reach neighbor from Trophallaxis Pass 1 reused by Sweep 1
/// Pass 2 so donors never re-scan 9 spatial bins, re-call `hypot`, or
/// recompute Pass-1 32-D genetic cosines (`24 B`, `alike < 0.0` = uncomputed).
#[derive(Debug, Clone, Copy, Default)]
struct CachedNeighbor {
    distance: f64,
    alike: f64,
    other: u32,
}

/// Reusable buffers for [`relax`].
#[derive(Debug, Default)]
pub struct ContactScratch {
    /// Start of each bin's members in `members`, plus one end.
    starts: Vec<u32>,
    /// Indices into the positions, grouped by bin, in index order.
    members: Vec<u32>,
    /// Each position's bin (`u32::MAX` for dead bodies).
    bins: Vec<u32>,
    /// Each body's push from the bodies it overlaps, unclamped, when the
    /// rules inhibit; empty otherwise.
    pressed: Vec<[f64; 2]>,
    /// Pass-1 donor export scale factors (`V2-13` `trophallaxis`; `0 B` when disabled).
    trophallaxis_scales: Vec<f64>,
    /// Per-body `(start, len)` span into `neighbor_cache` populated during
    /// Trophallaxis Pass 1 (`len == u32::MAX` falls back to the 9-bin scan;
    /// `0 B` when trophallaxis is disabled).
    neighbor_spans: Vec<(u32, u32)>,
    /// Bounded flat cache of in-reach neighbors shared from Trophallaxis Pass 1
    /// to Pass 2 (`0 B` when trophallaxis is disabled).
    neighbor_cache: Vec<CachedNeighbor>,
}

/// Which `field-ecology-v2` Sweep-1 buffers [`ContactWork::with_capacity_v2_features`]
/// reserves up front (`0 B` for every `false` flag, `INV-15`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContactV2Buffers {
    /// `V2-08` (`pcp-polarity`).
    pub pcp_polarity: bool,
    /// `V2-08` (`emt-gate`) or `V2-12` (`h_1` EMT adhesion modulation).
    pub emt_gate: bool,
    /// `V2-12` (`notch-delta`).
    pub notch_delta: bool,
    /// `V2-13` (`cadherin-4d`).
    pub cadherin: bool,
    /// `V2-13` (`trophallaxis`).
    pub trophallaxis: bool,
    /// `V2-13` (`velocity-alignment`).
    pub velocity_alignment: bool,
}

/// Optional per-tick `field-ecology-v2` parameters passed into
/// [`ContactWork::relax_v2_features`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ContactV2Sweep {
    /// `V2-08` (`pcp-polarity`).
    pub pcp: Option<PcpSweepParams>,
    /// `V2-08` (`emt-gate`) or `V2-12` (`h_1` EMT adhesion modulation).
    pub emt_gate: bool,
    /// `V2-12` (`notch-delta`).
    pub notch_delta: bool,
    /// `V2-13` (`cadherin-4d`).
    pub cadherin: bool,
    /// `V2-13` (`trophallaxis`).
    pub trophallaxis: Option<TrophallaxisSweepParams>,
    /// `V2-13` (`velocity-alignment`).
    pub velocity_alignment: Option<VelocityAlignmentSweepParams>,
}

/// A world's contact buffers: the survivors' gathered state, the
/// displacements and the bins, reserved once for the largest population.
#[derive(Debug, Default)]
pub struct ContactWork {
    /// Every working organism's settled position, in identity order.
    pub positions: Vec<[f64; 2]>,
    /// Their identities.
    pub ids: Vec<u64>,
    /// Whether each survives the tick.
    pub alive: Vec<bool>,
    /// Each one's unit affinity direction, when genetic affinity is needed
    /// (baseline adhesion without cadherin, PCP without cadherin, Notch-Delta,
    /// trophallaxis, or velocity alignment without cadherin).
    pub affinities: Vec<[f32; AFFINITY_TERMS]>,
    /// Committed unit polarity vectors `p_i` (`V2-08`; empty / `0 B` when off).
    pub polarities: Vec<[f64; 2]>,
    /// Updated unit polarity vectors `p'_i` from Sweep 1 (`V2-08`; empty / `0 B` when off).
    pub next_polarities: Vec<[f64; 2]>,
    /// Heritable lateral-belt polarity strengths `α_pol,i` (`V2-08`; empty / `0 B` when off).
    pub polarity_strengths: Vec<f64>,
    /// Heritable spontaneous apical wedge curvatures `β_i` (`V2-08`; empty / `0 B` when off).
    pub wedge_curvatures: Vec<f64>,
    /// Observed energy gradients `∇q_i` (`V2-08`; empty / `0 B` when off).
    pub energy_gradients: Vec<[f64; 2]>,
    /// Combined EMT adhesion multipliers (`V2-08` / `V2-12`; empty / `0 B` when off).
    pub emt_factors: Vec<f64>,
    /// Post-Phase-4 primary recurrent cell states `h_0` (`V2-12` `notch-delta`;
    /// empty / `0 B` when off).
    pub cell_states_h0: Vec<f32>,
    /// Kin-weighted mean neighbor cell states `h̄_0` computed on Sweep 1 (`V2-12`
    /// `notch-delta`; empty / `0 B` when off).
    pub neighbor_h0: Vec<f32>,
    /// Decoupled 4-D unit cadherin tags `c_i` (`V2-13` `cadherin-4d`; `0 B` when off).
    pub cadherin_tags: Vec<[f64; CADHERIN_TERMS]>,
    /// Heritable cadherin cohesiveness scalars `s_i` (`V2-13` `cadherin-4d`; `0 B` when off).
    pub cadherin_cohesiveness: Vec<f64>,
    /// Pre-trophallaxis survivor budgets `E_i` (`V2-13` `trophallaxis`; `0 B` when off).
    pub energies: Vec<f64>,
    /// Signed per-body exact binary-quantum deltas `Δk_i` (`V2-13` `trophallaxis`; `0 B` when off).
    pub trophallaxis_delta_quanta: Vec<i64>,
    /// Population-wide exact integer quantum totals (`V2-13` `trophallaxis`).
    pub trophallaxis_summary: TrophallaxisSummary,
    /// Committed post-motion velocities `v_i` (`V2-13` `velocity-alignment`; `0 B` when off).
    pub velocities: Vec<[f64; 2]>,
    /// Updated speed-non-increasing aligned velocities `v'_i` (`V2-13`; `0 B` when off).
    pub next_velocities: Vec<[f64; 2]>,
    /// Each one's displacement, zero for those that die: the sum of its
    /// sweeps'.
    pub displacements: Vec<[f64; 2]>,
    /// Each one's push from the bodies it overlapped, summed over the
    /// sweeps, when the rules inhibit; empty otherwise.
    pub pressed: Vec<[f64; 2]>,
    /// The settled positions, kept while later sweeps move `positions`.
    settled: Vec<[f64; 2]>,
    /// One sweep's displacements.
    sweep: Vec<[f64; 2]>,
    scratch: ContactScratch,
}

fn reserve<T>(buffer: &mut Vec<T>, rows: usize, name: &'static str) -> Result<(), CoreError> {
    buffer
        .try_reserve_exact(rows)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: name,
            bytes: u64::try_from(rows.saturating_mul(size_of::<T>())).unwrap_or(u64::MAX),
        })
}

fn bytes<T>(buffer: &Vec<T>) -> u64 {
    u64::try_from(buffer.capacity().saturating_mul(size_of::<T>())).unwrap_or(u64::MAX)
}

impl ContactWork {
    /// Buffers for `rows` organisms on `grid` under `rules`, reserved now so
    /// that no later tick allocates (FL-043).
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when a reservation fails.
    pub fn with_capacity(
        grid: &FieldGrid,
        rules: &ContactRules,
        rows: usize,
    ) -> Result<Self, CoreError> {
        Self::with_capacity_v2_features(grid, rules, rows, ContactV2Buffers::default())
    }

    /// Buffers for `rows` organisms on `grid` under `rules`, additionally
    /// reserving `V2-08` PCP polarity, `V2-08`/`V2-12` EMT adhesion multipliers,
    /// and/or `V2-12` Notch-Delta scratch ONLY when enabled (`0 B` when
    /// disabled, `INV-15`).
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when a reservation fails.
    pub fn with_capacity_v2(
        grid: &FieldGrid,
        rules: &ContactRules,
        rows: usize,
        pcp_polarity: bool,
        emt_gate: bool,
        notch_delta: bool,
    ) -> Result<Self, CoreError> {
        Self::with_capacity_v2_features(
            grid,
            rules,
            rows,
            ContactV2Buffers {
                pcp_polarity,
                emt_gate,
                notch_delta,
                ..ContactV2Buffers::default()
            },
        )
    }

    /// Buffers for `rows` organisms on `grid` under `rules`, reserving only
    /// those `field-ecology-v2` Sweep-1 scratch arrays whose feature flag in
    /// `buffers` is `true` (`0 B` when disabled, `INV-15`).
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when a reservation fails.
    pub fn with_capacity_v2_features(
        grid: &FieldGrid,
        rules: &ContactRules,
        rows: usize,
        buffers: ContactV2Buffers,
    ) -> Result<Self, CoreError> {
        // Later sweeps push only, binned at the diameter: never fewer bins.
        let reach = if rules.sweeps > 1 {
            rules.diameter
        } else if buffers.pcp_polarity
            || buffers.notch_delta
            || buffers.trophallaxis
            || buffers.velocity_alignment
        {
            rules.diameter * rules.range
        } else {
            rules.reach()
        };
        let bins = bins_along(grid.world_width(), reach) * bins_along(grid.world_height(), reach);
        let mut work = Self::default();
        reserve(&mut work.positions, rows, "contact positions")?;
        reserve(&mut work.ids, rows, "contact identities")?;
        reserve(&mut work.alive, rows, "contact fates")?;
        let needs_affinities = (rules.adheres() && !buffers.cadherin)
            || (buffers.pcp_polarity && !buffers.cadherin)
            || buffers.notch_delta
            || buffers.trophallaxis
            || (buffers.velocity_alignment && !buffers.cadherin);
        if needs_affinities {
            reserve(&mut work.affinities, rows, "contact affinities")?;
        }
        if buffers.pcp_polarity {
            reserve(&mut work.polarities, rows, "contact polarities")?;
            reserve(&mut work.next_polarities, rows, "contact next polarities")?;
            reserve(
                &mut work.polarity_strengths,
                rows,
                "contact polarity strengths",
            )?;
            reserve(&mut work.wedge_curvatures, rows, "contact wedge curvatures")?;
            reserve(&mut work.energy_gradients, rows, "contact energy gradients")?;
        }
        if buffers.emt_gate {
            reserve(&mut work.emt_factors, rows, "contact emt factors")?;
        }
        if buffers.notch_delta {
            reserve(&mut work.cell_states_h0, rows, "contact cell states h0")?;
            reserve(&mut work.neighbor_h0, rows, "contact neighbor h0")?;
        }
        if buffers.cadherin {
            reserve(&mut work.cadherin_tags, rows, "contact cadherin tags")?;
            reserve(
                &mut work.cadherin_cohesiveness,
                rows,
                "contact cadherin cohesiveness",
            )?;
        }
        if buffers.trophallaxis {
            reserve(&mut work.energies, rows, "contact trophallaxis energies")?;
            reserve(
                &mut work.trophallaxis_delta_quanta,
                rows,
                "contact trophallaxis deltas",
            )?;
            reserve(
                &mut work.scratch.trophallaxis_scales,
                rows,
                "contact trophallaxis scales",
            )?;
            reserve(
                &mut work.scratch.neighbor_spans,
                rows,
                "contact neighbor spans",
            )?;
            let cache_entries = rows.saturating_mul(24).min(1_048_576);
            reserve(
                &mut work.scratch.neighbor_cache,
                cache_entries,
                "contact neighbor cache",
            )?;
        }
        if buffers.velocity_alignment {
            reserve(&mut work.velocities, rows, "contact velocities")?;
            reserve(&mut work.next_velocities, rows, "contact next velocities")?;
        }
        reserve(&mut work.displacements, rows, "contact displacements")?;
        if rules.sweeps > 1 {
            reserve(&mut work.settled, rows, "contact settled positions")?;
            reserve(&mut work.sweep, rows, "contact sweep")?;
        }
        if rules.inhibits {
            reserve(&mut work.pressed, rows, "contact pushes")?;
            reserve(&mut work.scratch.pressed, rows, "contact sweep pushes")?;
        }
        reserve(&mut work.scratch.members, rows, "contact members")?;
        reserve(&mut work.scratch.bins, rows, "contact bins")?;
        reserve(&mut work.scratch.starts, bins + 1, "contact bin starts")?;
        Ok(work)
    }

    /// Bytes the buffers hold.
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        [
            bytes(&self.positions),
            bytes(&self.ids),
            bytes(&self.alive),
            bytes(&self.affinities),
            bytes(&self.polarities),
            bytes(&self.next_polarities),
            bytes(&self.polarity_strengths),
            bytes(&self.wedge_curvatures),
            bytes(&self.energy_gradients),
            bytes(&self.emt_factors),
            bytes(&self.cell_states_h0),
            bytes(&self.neighbor_h0),
            bytes(&self.cadherin_tags),
            bytes(&self.cadherin_cohesiveness),
            bytes(&self.energies),
            bytes(&self.trophallaxis_delta_quanta),
            bytes(&self.velocities),
            bytes(&self.next_velocities),
            bytes(&self.displacements),
            bytes(&self.pressed),
            bytes(&self.settled),
            bytes(&self.sweep),
            bytes(&self.scratch.members),
            bytes(&self.scratch.bins),
            bytes(&self.scratch.starts),
            bytes(&self.scratch.pressed),
            bytes(&self.scratch.trophallaxis_scales),
            bytes(&self.scratch.neighbor_spans),
            bytes(&self.scratch.neighbor_cache),
        ]
        .into_iter()
        .fold(0_u64, u64::saturating_add)
    }

    /// [`relax`] on the gathered survivors, `rules.sweeps` times, into
    /// [`Self::displacements`] (and [`Self::pressed`] when the rules
    /// inhibit), on up to `workers` threads. After more than one sweep
    /// [`Self::positions`] holds where each survivor ends.
    ///
    /// # Errors
    /// As [`relax`], and [`CoreError::NumericalInvalid`] for a position that
    /// cannot be wrapped.
    pub fn relax(
        &mut self,
        grid: &FieldGrid,
        rules: &ContactRules,
        workers: usize,
    ) -> Result<(), CoreError> {
        self.relax_v2_features(grid, rules, ContactV2Sweep::default(), workers)
    }

    /// [`Self::relax`] extended with optional `V2-08` PCP polarity (`pcp`),
    /// `V2-08`/`V2-12` EMT adhesion multipliers (`emt_gate`), and `V2-12`
    /// Notch-Delta neighbor averaging (`notch_delta`) on Sweep 1. When all
    /// three are disabled, runs the exact baseline isotropic [`relax`] path
    /// bit-for-bit. Sweeps `2..S` remain strictly push-only (`d < D`).
    ///
    /// # Errors
    /// As [`relax_sweep1_with_context`] and [`FieldGrid::wrap_position`].
    pub fn relax_v2(
        &mut self,
        grid: &FieldGrid,
        rules: &ContactRules,
        pcp: Option<PcpSweepParams>,
        emt_gate: bool,
        notch_delta: bool,
        workers: usize,
    ) -> Result<(), CoreError> {
        self.relax_v2_features(
            grid,
            rules,
            ContactV2Sweep {
                pcp,
                emt_gate,
                notch_delta,
                ..ContactV2Sweep::default()
            },
            workers,
        )
    }

    /// [`Self::relax`] extended with all optional `field-ecology-v2` Sweep-1
    /// hooks (`V2-08` PCP/EMT, `V2-12` Notch-Delta, `V2-13` cadherin-4d,
    /// passive exact-quantum trophallaxis, and dissipative velocity alignment).
    /// When all options are disabled, runs the exact baseline isotropic
    /// [`relax`] path bit-for-bit. Sweeps `2..S` remain strictly push-only.
    ///
    /// # Errors
    /// As [`relax_sweep1_with_context`] and [`FieldGrid::wrap_position`].
    pub fn relax_v2_features(
        &mut self,
        grid: &FieldGrid,
        rules: &ContactRules,
        sweep_v2: ContactV2Sweep,
        workers: usize,
    ) -> Result<(), CoreError> {
        let Self {
            positions,
            ids,
            alive,
            affinities,
            polarities,
            next_polarities,
            polarity_strengths,
            wedge_curvatures,
            energy_gradients,
            emt_factors,
            cell_states_h0,
            neighbor_h0,
            cadherin_tags,
            cadherin_cohesiveness,
            energies,
            trophallaxis_delta_quanta,
            trophallaxis_summary,
            velocities,
            next_velocities,
            displacements,
            pressed,
            settled,
            sweep,
            scratch,
        } = self;
        *trophallaxis_summary = TrophallaxisSummary::default();
        let ContactV2Sweep {
            pcp,
            emt_gate,
            notch_delta,
            cadherin,
            trophallaxis,
            velocity_alignment,
        } = sweep_v2;
        let has_v2 = pcp.is_some()
            || emt_gate
            || notch_delta
            || cadherin
            || trophallaxis.is_some()
            || velocity_alignment.is_some();
        let needs_affinities = (rules.adheres() && !cadherin)
            || (pcp.is_some() && !cadherin)
            || notch_delta
            || trophallaxis.is_some()
            || (velocity_alignment.is_some() && !cadherin);
        let sweep1_affinities = needs_affinities.then_some(affinities.as_slice());
        let ctx = Sweep1Context {
            pcp,
            polarities: pcp.map(|_| polarities.as_slice()),
            polarity_strengths: pcp.map(|_| polarity_strengths.as_slice()),
            wedge_curvatures: pcp.map(|_| wedge_curvatures.as_slice()),
            energy_gradients: pcp.map(|_| energy_gradients.as_slice()),
            emt_factors: emt_gate.then_some(emt_factors.as_slice()),
            cell_states_h0: notch_delta.then_some(cell_states_h0.as_slice()),
            cadherin_tags: cadherin.then_some(cadherin_tags.as_slice()),
            cadherin_cohesiveness: cadherin.then_some(cadherin_cohesiveness.as_slice()),
            trophallaxis,
            energies: trophallaxis.map(|_| energies.as_slice()),
            velocity_alignment,
            velocities: velocity_alignment.map(|_| velocities.as_slice()),
        };
        if rules.sweeps <= 1 {
            if has_v2 {
                relax_sweep1_with_context(
                    grid,
                    rules,
                    Bodies {
                        positions,
                        ids,
                        alive,
                        affinities: sweep1_affinities,
                    },
                    &ctx,
                    Sweep1Output {
                        displacements,
                        next_polarities: pcp.map(|_| next_polarities),
                        neighbor_h0: notch_delta.then_some(neighbor_h0),
                        trophallaxis_delta_quanta: trophallaxis.map(|_| trophallaxis_delta_quanta),
                        trophallaxis_summary: trophallaxis.map(|_| trophallaxis_summary),
                        next_velocities: velocity_alignment.map(|_| next_velocities),
                    },
                    scratch,
                    workers,
                )?;
            } else {
                relax(
                    grid,
                    rules,
                    Bodies {
                        positions,
                        ids,
                        alive,
                        affinities: sweep1_affinities,
                    },
                    displacements,
                    scratch,
                    workers,
                )?;
            }
            if rules.inhibits {
                std::mem::swap(pressed, &mut scratch.pressed);
            } else {
                pressed.clear();
            }
            return Ok(());
        }
        let count = positions.len();
        settled.clear();
        settled.extend_from_slice(positions);
        displacements.clear();
        displacements.resize(count, [0.0, 0.0]);
        pressed.clear();
        if rules.inhibits {
            pressed.resize(count, [0.0, 0.0]);
        }
        // Every sweep after the first pushes only.
        let pushing = ContactRules {
            adhesion: 0.0,
            range: 1.0,
            ..*rules
        };
        for index in 0..rules.sweeps {
            if index == 0 && has_v2 {
                relax_sweep1_with_context(
                    grid,
                    rules,
                    Bodies {
                        positions,
                        ids,
                        alive,
                        affinities: sweep1_affinities,
                    },
                    &ctx,
                    Sweep1Output {
                        displacements: sweep,
                        next_polarities: pcp.map(|_| &mut *next_polarities),
                        neighbor_h0: notch_delta.then_some(&mut *neighbor_h0),
                        trophallaxis_delta_quanta: trophallaxis
                            .map(|_| &mut *trophallaxis_delta_quanta),
                        trophallaxis_summary: trophallaxis.map(|_| &mut *trophallaxis_summary),
                        next_velocities: velocity_alignment.map(|_| &mut *next_velocities),
                    },
                    scratch,
                    workers,
                )?;
            } else {
                let (sweep_rules, sweep_affinities) = if index == 0 {
                    (rules, sweep1_affinities)
                } else {
                    (&pushing, None)
                };
                relax(
                    grid,
                    sweep_rules,
                    Bodies {
                        positions,
                        ids,
                        alive,
                        affinities: sweep_affinities,
                    },
                    sweep,
                    scratch,
                    workers,
                )?;
            }
            for (index, &[dx, dy]) in sweep.iter().enumerate() {
                if dx != 0.0 || dy != 0.0 {
                    let total = &mut displacements[index];
                    total[0] += dx;
                    total[1] += dy;
                    let from = settled[index];
                    positions[index] =
                        grid.wrap_position([from[0] + total[0], from[1] + total[1]])?;
                }
            }
            for (sum, push) in pressed.iter_mut().zip(&scratch.pressed) {
                sum[0] += push[0];
                sum[1] += push[1];
            }
        }
        Ok(())
    }
}

/// `affinity` as a unit direction in `f32`; zero for no direction.
#[must_use]
pub fn unit_affinity(affinity: &[f64; AFFINITY_TERMS]) -> [f32; AFFINITY_TERMS] {
    let norm = affinity
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    if norm.is_finite() && norm > 0.0 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a unit component, stored in f32 for the pairwise cosine"
        )]
        affinity.map(|value| (value / norm) as f32)
    } else {
        [0.0; AFFINITY_TERMS]
    }
}

/// The bins along one side: as many as fit cells at least `reach` wide.
fn bins_along(length: f64, reach: f64) -> usize {
    let fit = (length / reach).floor();
    if fit.is_finite() && fit >= 1.0 {
        // `fit` is a finite positive whole number; the clamp keeps it small.
        let fit = fit.min(MAXIMUM_BINS_PER_SIDE as f64);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a finite whole number in [1, 2048]"
        )]
        let bins = fit as usize;
        bins
    } else {
        1
    }
}

fn bin_of(coordinate: f64, length: f64, bins: usize) -> usize {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "a wrapped coordinate in [0, length) scaled into [0, bins]"
    )]
    let bin = (coordinate / length * bins as f64) as usize;
    bin.min(bins - 1)
}

/// The neighbour bins of `bin` along one side, without repeats when the side
/// has fewer than three bins.
fn neighbours(bin: usize, bins: usize) -> ([usize; 3], usize) {
    match bins {
        1 => ([0, 0, 0], 1),
        2 => ([bin ^ 1, bin, 0], 2),
        _ => ([(bin + bins - 1) % bins, bin, (bin + 1) % bins], 3),
    }
}

/// The cosine of two unit affinities, summed in `f64` in channel order.
#[inline]
fn cosine(a: &[f32; AFFINITY_TERMS], b: &[f32; AFFINITY_TERMS]) -> f64 {
    let mut sum = 0.0_f64;
    for (&x, &y) in a.iter().zip(b) {
        sum += f64::from(x) * f64::from(y);
    }
    sum
}

/// The bodies [`relax`] reads, aligned and in identity order.
#[derive(Debug, Clone, Copy)]
pub struct Bodies<'a> {
    /// Wrapped positions.
    pub positions: &'a [[f64; 2]],
    /// Identities, used only to separate coincident bodies.
    pub ids: &'a [u64],
    /// Whether each survives the tick; the others neither push nor move.
    pub alive: &'a [bool],
    /// Unit affinities ([`unit_affinity`]), required when bodies adhere.
    pub affinities: Option<&'a [[f32; AFFINITY_TERMS]]>,
}

/// The minimum-image difference `to − from` along a side of `length`, for
/// coordinates already checked to lie in `[0, length)`: the arithmetic of
/// [`flife_field::minimum_image`], without its check.
fn image(from: f64, to: f64, length: f64, half: f64) -> f64 {
    let difference = to - from;
    if difference >= half {
        difference - length
    } else if difference < -half {
        difference + length
    } else {
        difference
    }
}

/// Every survivor's contact displacement, written into `displacements`
/// (aligned with the bodies; zero where a body dies), the bodies split
/// among up to `workers` threads.
///
/// # Errors
/// [`CoreError::StateConflict`] for misaligned inputs, or no affinities when
/// the rules adhere; [`CoreError::NumericalInvalid`] for a position the grid
/// does not hold.
pub fn relax(
    grid: &FieldGrid,
    rules: &ContactRules,
    bodies: Bodies<'_>,
    displacements: &mut Vec<[f64; 2]>,
    scratch: &mut ContactScratch,
    workers: usize,
) -> Result<(), CoreError> {
    rules.check()?;
    let Bodies {
        positions,
        ids,
        alive,
        affinities,
    } = bodies;
    let count = positions.len();
    let affinities = match (rules.adheres(), affinities) {
        (true, Some(affinities)) if affinities.len() == count => Some(affinities),
        (true, _) => {
            return Err(CoreError::StateConflict {
                operation: "contact",
                detail: format!("adhesion needs one affinity per body, of {count}"),
            });
        }
        (false, _) => None,
    };
    if ids.len() != count || alive.len() != count {
        return Err(CoreError::StateConflict {
            operation: "contact",
            detail: format!(
                "{count} positions, {} identities and {} fates",
                ids.len(),
                alive.len()
            ),
        });
    }
    let ContactRules {
        diameter,
        stiffness,
        adhesion,
        range,
        inhibits,
        repulsion,
        ..
    } = *rules;
    let reach = rules.reach();
    let (width, height) = (grid.world_width(), grid.world_height());
    let (bins_x, bins_y) = (bins_along(width, reach), bins_along(height, reach));
    let limit = (0.5 * diameter).min(0.5 * grid.cell_width().min(grid.cell_height()));

    // Counting sort of the survivors by bin, stable in identity order.
    if count >= u32::MAX as usize {
        return Err(CoreError::Overflow {
            quantity: "contact bodies",
        });
    }
    let total_bins = bins_x * bins_y;
    let ContactScratch {
        starts,
        members,
        bins,
        pressed,
        trophallaxis_scales: _,
        neighbor_spans: _,
        neighbor_cache: _,
    } = scratch;
    starts.clear();
    starts.resize(total_bins + 1, 0);
    bins.clear();
    for (&[x, y], &live) in positions.iter().zip(alive) {
        let bin = if live {
            if !(0.0..width).contains(&x) || !(0.0..height).contains(&y) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "contact position",
                    detail: format!("[{x:?}, {y:?}] is not wrapped"),
                });
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "total_bins <= MAXIMUM_BINS_PER_SIDE^2 = 4_194_304 < u32::MAX"
            )]
            let b = (bin_of(y, height, bins_y) * bins_x + bin_of(x, width, bins_x)) as u32;
            b
        } else {
            u32::MAX
        };
        bins.push(bin);
        if bin != u32::MAX {
            starts[bin as usize] += 1;
        }
    }
    let mut running = 0_u32;
    for slot in &mut starts[..total_bins] {
        running += *slot;
        *slot = running;
    }
    starts[total_bins] = running;
    members.clear();
    members.resize(running as usize, 0);
    // Fill each bin from its end, walking the bodies backwards, so that
    // `starts[b]` ends as bin b's first member with no shift or memmove.
    for (index, &bin) in bins.iter().enumerate().rev() {
        if bin != u32::MAX {
            let b = bin as usize;
            starts[b] -= 1;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "count < u32::MAX checked above"
            )]
            let body = index as u32;
            members[starts[b] as usize] = body;
        }
    }

    displacements.clear();
    displacements.resize(count, [0.0, 0.0]);
    pressed.clear();
    if inhibits {
        pressed.resize(count, [0.0, 0.0]);
    }
    let push_factor = 0.5 * stiffness;
    let pull_factor = if range > 1.0 {
        0.5 * adhesion / ((range - 1.0) * diameter)
    } else {
        0.0
    };
    let reach_distance = diameter * range;
    // A pair whose squared distance is beyond this is beyond reach whatever
    // the rounding of its distance; nearer pairs are judged by the distance.
    let beyond = reach_distance * reach_distance * (1.0 + 1e-9);
    let (half_width, half_height) = (0.5 * width, 0.5 * height);
    let (starts, members, bins) = (&*starts, &*members, &*bins);
    // A body's clamped displacement and its unclamped push from the bodies
    // it overlaps.
    let sum = |index: usize| -> ([f64; 2], [f64; 2]) {
        let bin = bins[index];
        if bin == u32::MAX {
            return ([0.0, 0.0], [0.0, 0.0]);
        }
        let bin = bin as usize;
        let [here_x, here_y] = positions[index];
        let aff_i = affinities.map(|aff| &aff[index]);
        let (bx, by) = (bin % bins_x, bin / bins_x);
        let (rows, row_count) = neighbours(by, bins_y);
        let (columns, column_count) = neighbours(bx, bins_x);
        let mut sum = [0.0_f64, 0.0_f64];
        let mut overlapping = [0.0_f64, 0.0_f64];
        for &row in &rows[..row_count] {
            let row_base = row * bins_x;
            for &column in &columns[..column_count] {
                let neighbour = row_base + column;
                let lo = starts[neighbour] as usize;
                let hi = starts[neighbour + 1] as usize;
                for &other_u32 in &members[lo..hi] {
                    let other = other_u32 as usize;
                    if other == index {
                        continue;
                    }
                    // From the other body to this one.
                    let there = positions[other];
                    let dx = image(there[0], here_x, width, half_width);
                    if dx.abs() >= reach_distance {
                        continue;
                    }
                    let dy = image(there[1], here_y, height, half_height);
                    if dy.abs() >= reach_distance || dx * dx + dy * dy > beyond {
                        continue;
                    }
                    let distance = dx.hypot(dy);
                    let overlaps = distance < diameter;
                    let push = if overlaps {
                        push_factor * (diameter - distance)
                    } else if let (Some(affinities), Some(aff_self)) =
                        (affinities.filter(|_| distance < reach_distance), aff_i)
                    {
                        let alike = cosine(aff_self, &affinities[other]).max(0.0);
                        if repulsion > 0.0 {
                            let weight = (1.0 + repulsion) * (alike * alike) - repulsion;
                            -pull_factor
                                * weight
                                * (distance - diameter)
                                * (reach_distance - distance)
                        } else {
                            // The plain rule, in its own order of operations.
                            -pull_factor
                                * alike
                                * alike
                                * (distance - diameter)
                                * (reach_distance - distance)
                        }
                    } else {
                        continue;
                    };
                    let (ux, uy) = if distance > 0.0 {
                        (dx / distance, dy / distance)
                    } else if ids[index] > ids[other] {
                        (1.0, 0.0)
                    } else {
                        (-1.0, 0.0)
                    };
                    sum[0] += push * ux;
                    sum[1] += push * uy;
                    if overlaps {
                        overlapping[0] += push * ux;
                        overlapping[1] += push * uy;
                    }
                }
            }
        }
        let length = sum[0].hypot(sum[1]);
        if length > limit {
            let scale = limit / length;
            sum = [sum[0] * scale, sum[1] * scale];
        }
        (sum, overlapping)
    };
    // `pushes` is empty when the rules do not inhibit.
    let fill = |first: usize, out: &mut [[f64; 2]], pushes: &mut [[f64; 2]]| {
        for (offset, slot) in out.iter_mut().enumerate() {
            let (displacement, push) = sum(first + offset);
            *slot = displacement;
            if let Some(push_slot) = pushes.get_mut(offset) {
                *push_slot = push;
            }
        }
    };
    let used = workers
        .max(1)
        .min(count.div_ceil(MINIMUM_BODIES_PER_WORKER))
        .max(1);
    if used == 1 {
        fill(0, displacements, pressed);
        return Ok(());
    }
    let length = count.div_ceil(used);
    let mut chunks = displacements.chunks_mut(length);
    let mut push_chunks = pressed
        .chunks_mut(length)
        .chain(std::iter::repeat_with(|| -> &mut [[f64; 2]] { &mut [] }));
    let head = chunks.next().unwrap_or_default();
    let push_head = push_chunks.next().unwrap_or_default();
    let fill = &fill;
    flife_core::pool::scope_workers(workers, "contact", |scope| {
        for (index, (chunk, pushes)) in chunks.zip(push_chunks).enumerate() {
            scope.spawn(move || fill((index + 1) * length, chunk, pushes));
        }
        fill(0, head, push_head);
    })
}

/// Sweep 1 with optional `V2-08` PCP lateral/wedge adhesion, non-vanishing
/// equilibrium polarity torque, neural/epigenetic EMT adhesion gate, `V2-12`
/// Notch-Delta neighbor averaging, and `V2-13` decoupled 4-D cadherin,
/// passive exact-quantum (`2^-30`) kin trophallaxis, and dissipative velocity
/// alignment.
///
/// When all optional `context` fields are `None`, delegates directly to
/// [`relax`] bit-for-bit.
///
/// # Errors
/// [`CoreError::StateConflict`] for misaligned inputs, missing required
/// buffers, or quantum-conservation mismatch; [`CoreError::NumericalInvalid`]
/// for unwrapped positions, zero-norm cadherin tags, or nonfinite inputs.
#[expect(
    clippy::too_many_lines,
    reason = "Sweep 1 spatial bin pass combining soft-core overlap, V2-08 PCP/EMT, V2-12 Notch-Delta, and V2-13 cadherin/trophallaxis/velocity-alignment in one deterministic traversal"
)]
pub fn relax_sweep1_with_context(
    grid: &FieldGrid,
    rules: &ContactRules,
    bodies: Bodies<'_>,
    context: &Sweep1Context<'_>,
    output: Sweep1Output<'_>,
    scratch: &mut ContactScratch,
    workers: usize,
) -> Result<(), CoreError> {
    if context.pcp.is_none()
        && context.emt_factors.is_none()
        && context.cell_states_h0.is_none()
        && context.cadherin_tags.is_none()
        && context.cadherin_cohesiveness.is_none()
        && context.trophallaxis.is_none()
        && context.velocity_alignment.is_none()
    {
        return relax(grid, rules, bodies, output.displacements, scratch, workers);
    }
    rules.check()?;
    let Bodies {
        positions,
        ids,
        alive,
        affinities,
    } = bodies;
    let count = positions.len();
    if ids.len() != count || alive.len() != count {
        return Err(CoreError::StateConflict {
            operation: "contact",
            detail: format!(
                "{count} positions, {} identities and {} fates",
                ids.len(),
                alive.len()
            ),
        });
    }
    let cadherin_inputs = match (context.cadherin_tags, context.cadherin_cohesiveness) {
        (Some(tags), Some(cohesions)) => {
            if tags.len() != count || cohesions.len() != count {
                return Err(CoreError::StateConflict {
                    operation: "contact",
                    detail: format!(
                        "cadherin-4d buffers misaligned for {count} bodies: tags={}, cohesions={}",
                        tags.len(),
                        cohesions.len()
                    ),
                });
            }
            for (index, (tag, &cohesion)) in tags.iter().zip(cohesions).enumerate() {
                if tag.iter().any(|c| !c.is_finite()) {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "cadherin tag",
                        detail: format!("body {index} has nonfinite cadherin tag {tag:?}"),
                    });
                }
                let norm_sq = tag[0] * tag[0] + tag[1] * tag[1] + tag[2] * tag[2] + tag[3] * tag[3];
                if !norm_sq.is_finite() || norm_sq <= 0.0 || norm_sq > 1.0 + 1.0e-12 {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "cadherin tag",
                        detail: format!(
                            "body {index} has invalid cadherin tag norm^2 {norm_sq:?} (must lie in (0, 1])"
                        ),
                    });
                }
                if !cohesion.is_finite() || !(0.0..=1.0).contains(&cohesion) {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "cadherin cohesiveness",
                        detail: format!(
                            "body {index} has invalid cohesiveness {cohesion:?} (must lie in [0, 1])"
                        ),
                    });
                }
            }
            Some((tags, cohesions))
        }
        (None, None) => None,
        _ => {
            return Err(CoreError::StateConflict {
                operation: "contact",
                detail: "cadherin-4d Sweep1Context requires both cadherin_tags and cadherin_cohesiveness"
                    .to_owned(),
            });
        }
    };
    let pcp_inputs = if let Some(pcp) = context.pcp {
        if !(pcp.dt.is_finite()
            && pcp.dt >= 0.0
            && pcp.torque_mobility.is_finite()
            && pcp.torque_mobility >= 0.0
            && pcp.gradient_cue_strength.is_finite()
            && pcp.gradient_cue_strength >= 0.0)
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "v2.polarity",
                detail: format!("invalid PCP sweep params {pcp:?}"),
            });
        }
        let (Some(polarities), Some(strengths), Some(curvatures), Some(gradients)) = (
            context.polarities,
            context.polarity_strengths,
            context.wedge_curvatures,
            context.energy_gradients,
        ) else {
            return Err(CoreError::StateConflict {
                operation: "contact",
                detail: "pcp-polarity Sweep1Context is missing required polarity buffers"
                    .to_owned(),
            });
        };
        if polarities.len() != count
            || strengths.len() != count
            || curvatures.len() != count
            || gradients.len() != count
        {
            return Err(CoreError::StateConflict {
                operation: "contact",
                detail: format!(
                    "pcp-polarity buffers misaligned for {count} bodies: p={}, alpha={}, beta={}, grad={}",
                    polarities.len(),
                    strengths.len(),
                    curvatures.len(),
                    gradients.len()
                ),
            });
        }
        if output.next_polarities.is_none() {
            return Err(CoreError::StateConflict {
                operation: "contact",
                detail: "pcp-polarity Sweep1Output requires next_polarities buffer".to_owned(),
            });
        }
        Some((pcp, polarities, strengths, curvatures, gradients))
    } else {
        None
    };
    let emt_factors = match context.emt_factors {
        Some(factors) if factors.len() == count => {
            for (index, &value) in factors.iter().enumerate() {
                if !value.is_finite() || value < 0.0 {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "emt adhesion multiplier",
                        detail: format!("body {index} has invalid emt_eff {value:?}"),
                    });
                }
            }
            Some(factors)
        }
        Some(factors) => {
            return Err(CoreError::StateConflict {
                operation: "contact",
                detail: format!("{} emt factors for {count} bodies", factors.len()),
            });
        }
        None => None,
    };
    let notch_h0 = match context.cell_states_h0 {
        Some(states) if states.len() == count => {
            for (index, &value) in states.iter().enumerate() {
                if !value.is_finite() || !(-1.0_f32..=1.0_f32).contains(&value) {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "notch-delta cell state h0",
                        detail: format!("body {index} has invalid h0 {value:?} outside [-1, 1]"),
                    });
                }
            }
            if output.neighbor_h0.is_none() {
                return Err(CoreError::StateConflict {
                    operation: "contact",
                    detail: "notch-delta Sweep1Output requires neighbor_h0 buffer".to_owned(),
                });
            }
            Some(states)
        }
        Some(states) => {
            return Err(CoreError::StateConflict {
                operation: "contact",
                detail: format!("{} cell_states_h0 entries for {count} bodies", states.len()),
            });
        }
        None => None,
    };
    let trophallaxis_inputs = if let Some(tr) = context.trophallaxis {
        if !(tr.dt.is_finite()
            && tr.dt >= 0.0
            && tr.conductance_per_second.is_finite()
            && tr.conductance_per_second >= 0.0
            && tr.transfer_efficiency.is_finite()
            && tr.transfer_efficiency > 0.0
            && tr.transfer_efficiency <= 0.99
            && tr.reserve_energy.is_finite()
            && tr.reserve_energy >= 0.0
            && tr.maximum_export_fraction.is_finite()
            && tr.maximum_export_fraction > 0.0
            && tr.maximum_export_fraction <= 1.0
            && tr.kin_threshold.is_finite()
            && (0.0..=1.0).contains(&tr.kin_threshold))
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "v2.trophallaxis",
                detail: format!("invalid trophallaxis sweep params {tr:?}"),
            });
        }
        let Some(energies) = context.energies else {
            return Err(CoreError::StateConflict {
                operation: "contact",
                detail: "trophallaxis Sweep1Context requires energies buffer".to_owned(),
            });
        };
        if energies.len() != count {
            return Err(CoreError::StateConflict {
                operation: "contact",
                detail: format!("{} energies for {count} bodies", energies.len()),
            });
        }
        for (index, &energy) in energies.iter().enumerate() {
            if !energy.is_finite() || energy < 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity: "trophallaxis energy",
                    detail: format!("body {index} has invalid energy {energy:?}"),
                });
            }
        }
        if output.trophallaxis_delta_quanta.is_none() || output.trophallaxis_summary.is_none() {
            return Err(CoreError::StateConflict {
                operation: "contact",
                detail: "trophallaxis Sweep1Output requires trophallaxis_delta_quanta and trophallaxis_summary"
                    .to_owned(),
            });
        }
        Some((tr, energies))
    } else {
        None
    };
    let velocity_inputs = if let Some(va) = context.velocity_alignment {
        if !va.strength.is_finite() || !(0.0..=0.4).contains(&va.strength) {
            return Err(CoreError::NumericalInvalid {
                quantity: "v2.contact.velocity_alignment.strength",
                detail: format!("invalid velocity alignment strength {:?}", va.strength),
            });
        }
        let Some(velocities) = context.velocities else {
            return Err(CoreError::StateConflict {
                operation: "contact",
                detail: "velocity-alignment Sweep1Context requires velocities buffer".to_owned(),
            });
        };
        if velocities.len() != count {
            return Err(CoreError::StateConflict {
                operation: "contact",
                detail: format!("{} velocities for {count} bodies", velocities.len()),
            });
        }
        for (index, &vel) in velocities.iter().enumerate() {
            if !(vel[0].is_finite() && vel[1].is_finite()) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "velocity alignment input",
                    detail: format!("body {index} has nonfinite velocity {vel:?}"),
                });
            }
        }
        if output.next_velocities.is_none() {
            return Err(CoreError::StateConflict {
                operation: "contact",
                detail: "velocity-alignment Sweep1Output requires next_velocities buffer"
                    .to_owned(),
            });
        }
        Some((va, velocities))
    } else {
        None
    };

    let has_cadherin = cadherin_inputs.is_some();
    let needs_affinities = (rules.adheres() && !has_cadherin)
        || (pcp_inputs.is_some() && !has_cadherin)
        || notch_h0.is_some()
        || trophallaxis_inputs.is_some()
        || (velocity_inputs.is_some() && !has_cadherin);
    let affinities = match (needs_affinities, affinities) {
        (true, Some(affinities)) if affinities.len() == count => Some(affinities),
        (true, _) => {
            return Err(CoreError::StateConflict {
                operation: "contact",
                detail: format!(
                    "adhesion/pcp/notch-delta/trophallaxis/velocity-alignment needs one affinity per body, of {count}"
                ),
            });
        }
        (false, _) => None,
    };

    let ContactRules {
        diameter,
        stiffness,
        adhesion,
        range,
        inhibits,
        repulsion,
        ..
    } = *rules;
    let adheres = rules.adheres();
    let reach = if adheres
        || pcp_inputs.is_some()
        || notch_h0.is_some()
        || trophallaxis_inputs.is_some()
        || velocity_inputs.is_some()
    {
        diameter * range
    } else {
        diameter
    };
    let (width, height) = (grid.world_width(), grid.world_height());
    let (bins_x, bins_y) = (bins_along(width, reach), bins_along(height, reach));
    let limit = (0.5 * diameter).min(0.5 * grid.cell_width().min(grid.cell_height()));

    // Counting sort of survivors by bin, stable in identity order.
    if count >= u32::MAX as usize {
        return Err(CoreError::Overflow {
            quantity: "contact bodies",
        });
    }
    let total_bins = bins_x * bins_y;
    let ContactScratch {
        starts,
        members,
        bins,
        pressed,
        trophallaxis_scales,
        neighbor_spans,
        neighbor_cache,
    } = scratch;
    starts.clear();
    starts.resize(total_bins + 1, 0);
    bins.clear();
    for (&[x, y], &live) in positions.iter().zip(alive) {
        let bin = if live {
            if !(0.0..width).contains(&x) || !(0.0..height).contains(&y) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "contact position",
                    detail: format!("[{x:?}, {y:?}] is not wrapped"),
                });
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "total_bins <= MAXIMUM_BINS_PER_SIDE^2 = 4_194_304 < u32::MAX"
            )]
            let b = (bin_of(y, height, bins_y) * bins_x + bin_of(x, width, bins_x)) as u32;
            b
        } else {
            u32::MAX
        };
        bins.push(bin);
        if bin != u32::MAX {
            starts[bin as usize] += 1;
        }
    }
    let mut running = 0_u32;
    for slot in &mut starts[..total_bins] {
        running += *slot;
        *slot = running;
    }
    starts[total_bins] = running;
    members.clear();
    members.resize(running as usize, 0);
    for (index, &bin) in bins.iter().enumerate().rev() {
        if bin != u32::MAX {
            let b = bin as usize;
            starts[b] -= 1;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "count < u32::MAX checked above"
            )]
            let body = index as u32;
            members[starts[b] as usize] = body;
        }
    }

    let Sweep1Output {
        displacements,
        mut next_polarities,
        mut neighbor_h0,
        mut trophallaxis_delta_quanta,
        mut trophallaxis_summary,
        mut next_velocities,
    } = output;
    displacements.clear();
    displacements.resize(count, [0.0, 0.0]);
    pressed.clear();
    if inhibits {
        pressed.resize(count, [0.0, 0.0]);
    }
    if let Some(pol_out) = next_polarities.as_deref_mut() {
        pol_out.clear();
        if let Some((_, polarities, _, _, _)) = pcp_inputs {
            pol_out.extend_from_slice(polarities);
        } else {
            pol_out.resize(count, [1.0, 0.0]);
        }
    }
    if let Some(notch_out) = neighbor_h0.as_deref_mut() {
        notch_out.clear();
        notch_out.resize(count, 0.0_f32);
    }
    if let Some(delta_out) = trophallaxis_delta_quanta.as_deref_mut() {
        delta_out.clear();
        delta_out.resize(count, 0_i64);
    }
    if let Some(summary_out) = trophallaxis_summary.as_mut() {
        **summary_out = TrophallaxisSummary::default();
    }
    if let Some(vel_out) = next_velocities.as_deref_mut() {
        vel_out.clear();
        if let Some((_, vels)) = velocity_inputs {
            vel_out.extend_from_slice(vels);
        } else {
            vel_out.resize(count, [0.0, 0.0]);
        }
    }

    let push_factor = 0.5 * stiffness;
    let pull_factor = if adheres {
        0.5 * adhesion / ((range - 1.0) * diameter)
    } else {
        0.0
    };
    let reach_distance = reach;
    let beyond = reach_distance * reach_distance * (1.0 + 1e-9);
    let (half_width, half_height) = (0.5 * width, 0.5 * height);
    let (starts, members, bins) = (&*starts, &*members, &*bins);
    let used = workers
        .max(1)
        .min(count.div_ceil(MINIMUM_BODIES_PER_WORKER))
        .max(1);

    // Trophallaxis Pass 1 (donor export cap, INV-01 / INV-05): compute each
    // donor's scale factor `scale_i in [0, 1]` over the immutable Sweep-1 bins,
    // and populate the bounded per-worker neighbor cache so Pass 2 never
    // re-scans 9 spatial bins, re-calls `hypot`, or recomputes Pass-1 32-D
    // cosines for cached donors.
    trophallaxis_scales.clear();
    neighbor_spans.clear();
    let mut has_neighbor_cache = false;
    if let Some((tr_cfg, energies)) = trophallaxis_inputs {
        let aff = affinities.ok_or_else(|| CoreError::StateConflict {
            operation: "contact",
            detail: "trophallaxis requires genetic affinities".to_owned(),
        })?;
        trophallaxis_scales.resize(count, 0.0_f64);
        neighbor_spans.resize(count, (0_u32, u32::MAX));
        let cache_cap = neighbor_cache.capacity();
        if neighbor_cache.len() < cache_cap {
            neighbor_cache.resize(cache_cap, CachedNeighbor::default());
        }
        has_neighbor_cache = cache_cap > 0;
        let fill_scales_and_cache = |first: usize,
                                     out_scales: &mut [f64],
                                     out_spans: &mut [(u32, u32)],
                                     cache_base: usize,
                                     cache_slice: &mut [CachedNeighbor]| {
            let mut cursor = 0_usize;
            let cache_len = cache_slice.len();
            for (offset, (scale_slot, span_slot)) in
                out_scales.iter_mut().zip(out_spans.iter_mut()).enumerate()
            {
                let index = first + offset;
                let bin = bins[index];
                if bin == u32::MAX {
                    *scale_slot = 0.0;
                    *span_slot = (0, u32::MAX);
                    continue;
                }
                let e_i = energies[index];
                let cap = tr_cfg.maximum_export_fraction * (e_i - tr_cfg.reserve_energy).max(0.0);
                if cap <= 0.0 {
                    *scale_slot = 0.0;
                    *span_slot = (0, u32::MAX);
                    continue;
                }
                let bin = bin as usize;
                let [here_x, here_y] = positions[index];
                let aff_i = &aff[index];
                let (bx, by) = (bin % bins_x, bin / bins_x);
                let (rows, row_count) = neighbours(by, bins_y);
                let (columns, column_count) = neighbours(bx, bins_x);
                let mut out_sum = CompensatedSum::ZERO;
                let span_start = cursor;
                let mut can_cache = cache_len > 0;
                for &row in &rows[..row_count] {
                    let row_base = row * bins_x;
                    for &column in &columns[..column_count] {
                        let neighbour = row_base + column;
                        let lo = starts[neighbour] as usize;
                        let hi = starts[neighbour + 1] as usize;
                        for &other_u32 in &members[lo..hi] {
                            let other = other_u32 as usize;
                            if other == index {
                                continue;
                            }
                            let there = positions[other];
                            let dx = image(there[0], here_x, width, half_width);
                            if dx.abs() >= reach_distance {
                                continue;
                            }
                            let dy = image(there[1], here_y, height, half_height);
                            if dy.abs() >= reach_distance || dx * dx + dy * dy > beyond {
                                continue;
                            }
                            let distance = dx.abs().hypot(dy.abs());
                            let in_reach = distance < reach_distance;
                            let overlaps = distance < diameter;
                            if !overlaps && !in_reach {
                                continue;
                            }
                            let e_j = energies[other];
                            let alike = if in_reach && e_j < e_i {
                                let a = cosine(aff_i, &aff[other]).max(0.0);
                                let raw = pair_trophallaxis_raw(
                                    e_i,
                                    e_j,
                                    distance,
                                    reach_distance,
                                    a,
                                    &tr_cfg,
                                );
                                if raw > 0.0 {
                                    out_sum.add(raw);
                                }
                                a
                            } else {
                                -1.0
                            };
                            if can_cache {
                                if cursor < cache_len {
                                    cache_slice[cursor] = CachedNeighbor {
                                        distance,
                                        alike,
                                        other: other_u32,
                                    };
                                    cursor += 1;
                                } else {
                                    cursor = span_start;
                                    can_cache = false;
                                }
                            }
                        }
                    }
                }
                if can_cache {
                    let span_len = cursor - span_start;
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "neighbor_cache length is capped at 1_048_576 < u32::MAX"
                    )]
                    {
                        *span_slot = ((cache_base + span_start) as u32, span_len as u32);
                    }
                } else {
                    *span_slot = (0, u32::MAX);
                }
                let total_out = out_sum.value();
                if total_out <= 0.0 {
                    *scale_slot = 0.0;
                } else {
                    let mut scale = (cap / total_out).min(1.0);
                    while scale > 0.0 && scale * total_out > cap {
                        scale = scale.next_down();
                    }
                    *scale_slot = scale;
                }
            }
        };
        if used == 1 {
            fill_scales_and_cache(
                0,
                trophallaxis_scales,
                neighbor_spans,
                0,
                neighbor_cache.as_mut_slice(),
            );
        } else {
            let length = count.div_ceil(used);
            let chunk_count = count.div_ceil(length);
            let cache_per_worker = neighbor_cache.len() / chunk_count.max(1);
            let mut scale_chunks = trophallaxis_scales.chunks_mut(length);
            let mut span_chunks = neighbor_spans.chunks_mut(length);
            let mut cache_rest = neighbor_cache.as_mut_slice();
            let take_head = cache_per_worker.min(cache_rest.len());
            let (cache_head, tail_after_head) = cache_rest.split_at_mut(take_head);
            cache_rest = tail_after_head;
            let scale_head = scale_chunks.next().unwrap_or_default();
            let span_head = span_chunks.next().unwrap_or_default();
            let fill_scales_and_cache = &fill_scales_and_cache;
            flife_core::pool::scope_workers(workers, "contact", |scope| {
                for (index, (s_chunk, sp_chunk)) in scale_chunks.zip(span_chunks).enumerate() {
                    let worker_idx = index + 1;
                    let take = cache_per_worker.min(cache_rest.len());
                    let (c_chunk, next_rest) = core::mem::take(&mut cache_rest).split_at_mut(take);
                    cache_rest = next_rest;
                    let c_base = worker_idx * cache_per_worker;
                    scope.spawn(move || {
                        fill_scales_and_cache(
                            worker_idx * length,
                            s_chunk,
                            sp_chunk,
                            c_base,
                            c_chunk,
                        );
                    });
                }
                fill_scales_and_cache(0, scale_head, span_head, 0, cache_head);
            })?;
        }
    }
    let tr_scales = trophallaxis_scales.as_slice();
    let tr_spans = neighbor_spans.as_slice();
    let tr_cache = neighbor_cache.as_slice();

    type BodyEval = (
        [f64; 2],
        [f64; 2],
        [f64; 2],
        f32,
        i64,
        TrophallaxisSummary,
        [f64; 2],
    );
    let eval_body = |index: usize| -> Result<BodyEval, CoreError> {
        let current_pol =
            pcp_inputs.map_or([1.0, 0.0], |(_, polarities, _, _, _)| polarities[index]);
        let current_vel = velocity_inputs.map_or([0.0, 0.0], |(_, vels)| vels[index]);
        let bin = bins[index];
        if bin == u32::MAX {
            return Ok((
                [0.0, 0.0],
                [0.0, 0.0],
                current_pol,
                0.0_f32,
                0_i64,
                TrophallaxisSummary::default(),
                current_vel,
            ));
        }
        let bin = bin as usize;
        let [here_x, here_y] = positions[index];
        let aff_i = affinities.map(|aff| &aff[index]);
        let mut sum = [0.0_f64, 0.0_f64];
        let mut overlapping = [0.0_f64, 0.0_f64];
        let mut tau_neighbors = 0.0_f64;
        let mut notch_num = 0.0_f64;
        let mut notch_den = 0.0_f64;
        let mut delta_k = 0_i64;
        let mut body_out = 0_u64;
        let mut body_in = 0_u64;
        let mut body_tax = 0_u64;
        let mut align_v = [0.0_f64, 0.0_f64];
        let mut align_w = 0.0_f64;
        let has_pcp = pcp_inputs.is_some();
        let has_vel = velocity_inputs.is_some();
        let mut process_pair = |other: usize,
                                dx: f64,
                                dy: f64,
                                distance: f64,
                                precomputed_alike: Option<f64>|
         -> Result<(), CoreError> {
            let overlaps = distance < diameter;
            let in_reach = distance < reach_distance;
            if !overlaps && !in_reach {
                return Ok(());
            }
            let (ux, uy) = if distance > 0.0 {
                (dx / distance, dy / distance)
            } else if ids[index] > ids[other] {
                (1.0, 0.0)
            } else {
                (-1.0, 0.0)
            };
            // `r_hat = (x_j - x_i) / d = -u_ij` for `d > 0`.
            let r_hat = [-ux, -uy];
            // Reuse Pass-1 32-D cosine when cached, otherwise evaluate lazily
            // only when an active consumer actually needs it.
            let mut cached_genetic_alike = precomputed_alike;
            let mut get_genetic_alike = || -> Option<f64> {
                if let Some(cached) = cached_genetic_alike {
                    Some(cached)
                } else {
                    let val = affinities
                        .zip(aff_i)
                        .map(|(aff, a_i)| cosine(a_i, &aff[other]).max(0.0));
                    cached_genetic_alike = val;
                    val
                }
            };
            let kin_weight = |alike: f64| -> f64 {
                if repulsion > 0.0 {
                    (1.0 + repulsion) * (alike * alike) - repulsion
                } else {
                    alike * alike
                }
            };
            let need_adhesion = (!overlaps && adheres && in_reach)
                || (in_reach && ((has_pcp && distance > 0.0) || has_vel));
            let adhesion_weight = if need_adhesion {
                match cadherin_inputs {
                    Some((tags, cohesions)) => cadherin_sorting_weight(
                        &tags[index],
                        cohesions[index],
                        &tags[other],
                        cohesions[other],
                        repulsion,
                    ),
                    None => get_genetic_alike().map_or(0.0, kin_weight),
                }
            } else {
                0.0
            };
            let adhesion_kin_pos = adhesion_weight.max(0.0);
            if overlaps {
                let push = push_factor * (diameter - distance);
                sum[0] += push * ux;
                sum[1] += push * uy;
                overlapping[0] += push * ux;
                overlapping[1] += push * uy;
            } else if adheres && in_reach {
                let eff_weight = if adhesion_weight > 0.0 {
                    let emt_pair = emt_factors
                        .map_or(1.0, |factors| (factors[index] * factors[other]).sqrt());
                    let pcp_factor = match pcp_inputs {
                        Some((_, polarities, strengths, curvatures, _)) => {
                            let alpha_ij = (strengths[index] * strengths[other]).sqrt();
                            let beta_bar = 0.5 * (curvatures[index] + curvatures[other]);
                            pcp_lateral_wedge_weight(
                                polarities[index],
                                polarities[other],
                                r_hat,
                                alpha_ij,
                                beta_bar,
                            )
                        }
                        None => 1.0,
                    };
                    emt_pair * adhesion_weight * pcp_factor
                } else {
                    adhesion_weight
                };
                let push = -pull_factor
                    * eff_weight
                    * (distance - diameter)
                    * (reach_distance - distance);
                sum[0] += push * ux;
                sum[1] += push * uy;
            }
            if let Some((_, polarities, strengths, curvatures, _)) = pcp_inputs
                && distance > 0.0
                && in_reach
                && adhesion_kin_pos > 0.0
            {
                let w_torque = adhesion_kin_pos * (1.0 - distance / reach_distance);
                let alpha_ij = (strengths[index] * strengths[other]).sqrt();
                let beta_bar = 0.5 * (curvatures[index] + curvatures[other]);
                let pair_term = pcp_pair_torque_term(
                    polarities[index],
                    polarities[other],
                    r_hat,
                    beta_bar,
                );
                tau_neighbors += alpha_ij * w_torque * pair_term;
            }
            if let Some(states_h0) = notch_h0
                && in_reach
            {
                let kin_pos = get_genetic_alike().map_or(0.0, kin_weight).max(0.0);
                if kin_pos > 0.0 {
                    notch_num += kin_pos * f64::from(states_h0[other]);
                    notch_den += kin_pos;
                }
            }
            if let Some((tr_cfg, energies)) = trophallaxis_inputs
                && in_reach
            {
                let e_i = energies[index];
                let e_j = energies[other];
                if e_i > e_j && tr_scales[index] > 0.0 {
                    let alike = get_genetic_alike().unwrap_or(0.0);
                    let (q_out, _q_in, q_tax) = pair_trophallaxis_quanta(
                        e_i,
                        tr_scales[index],
                        e_j,
                        distance,
                        reach_distance,
                        alike,
                        &tr_cfg,
                    );
                    if q_out > 0 {
                        delta_k = delta_k.checked_sub(q_out).ok_or(CoreError::Overflow {
                            quantity: "trophallaxis organism delta quanta",
                        })?;
                        let q_out_u = u64::try_from(q_out).map_err(|_| CoreError::Overflow {
                            quantity: "trophallaxis outgoing quanta",
                        })?;
                        let q_tax_u = u64::try_from(q_tax).map_err(|_| CoreError::Overflow {
                            quantity: "trophallaxis tax quanta",
                        })?;
                        body_out = body_out.checked_add(q_out_u).ok_or(CoreError::Overflow {
                            quantity: "trophallaxis outgoing quanta",
                        })?;
                        body_tax = body_tax.checked_add(q_tax_u).ok_or(CoreError::Overflow {
                            quantity: "trophallaxis tax quanta",
                        })?;
                    }
                } else if e_j > e_i && tr_scales[other] > 0.0 {
                    let alike = get_genetic_alike().unwrap_or(0.0);
                    let (_q_out, q_in, _q_tax) = pair_trophallaxis_quanta(
                        e_j,
                        tr_scales[other],
                        e_i,
                        distance,
                        reach_distance,
                        alike,
                        &tr_cfg,
                    );
                    if q_in > 0 {
                        delta_k = delta_k.checked_add(q_in).ok_or(CoreError::Overflow {
                            quantity: "trophallaxis organism delta quanta",
                        })?;
                        let q_in_u = u64::try_from(q_in).map_err(|_| CoreError::Overflow {
                            quantity: "trophallaxis incoming quanta",
                        })?;
                        body_in = body_in.checked_add(q_in_u).ok_or(CoreError::Overflow {
                            quantity: "trophallaxis incoming quanta",
                        })?;
                    }
                }
            }
            if let Some((_, vels)) = velocity_inputs
                && in_reach
                && adhesion_kin_pos > 0.0
            {
                align_v[0] += adhesion_kin_pos * vels[other][0];
                align_v[1] += adhesion_kin_pos * vels[other][1];
                align_w += adhesion_kin_pos;
            }
            Ok(())
        };

        let cached_span = if has_neighbor_cache {
            tr_spans.get(index).copied()
        } else {
            None
        };
        if let Some((span_start, span_len)) = cached_span
            && span_len != u32::MAX
        {
            let s = span_start as usize;
            let e = s + span_len as usize;
            for entry in &tr_cache[s..e] {
                let other = entry.other as usize;
                let there = positions[other];
                let dx = image(there[0], here_x, width, half_width);
                let dy = image(there[1], here_y, height, half_height);
                let pre_alike = (entry.alike >= 0.0).then_some(entry.alike);
                process_pair(other, dx, dy, entry.distance, pre_alike)?;
            }
        } else {
            let (bx, by) = (bin % bins_x, bin / bins_x);
            let (rows, row_count) = neighbours(by, bins_y);
            let (columns, column_count) = neighbours(bx, bins_x);
            for &row in &rows[..row_count] {
                let row_base = row * bins_x;
                for &column in &columns[..column_count] {
                    let neighbour = row_base + column;
                    let lo = starts[neighbour] as usize;
                    let hi = starts[neighbour + 1] as usize;
                    for &other_u32 in &members[lo..hi] {
                        let other = other_u32 as usize;
                        if other == index {
                            continue;
                        }
                        // From the other body to this one (`u_ij = (x_i - x_j) / d`).
                        let there = positions[other];
                        let dx = image(there[0], here_x, width, half_width);
                        if dx.abs() >= reach_distance {
                            continue;
                        }
                        let dy = image(there[1], here_y, height, half_height);
                        if dy.abs() >= reach_distance || dx * dx + dy * dy > beyond {
                            continue;
                        }
                        // Commutative distance `|dx|.hypot(|dy|)` guarantees bit-identical
                        // `distance` on both sides of pair `(i, j)`.
                        let distance = dx.abs().hypot(dy.abs());
                        process_pair(other, dx, dy, distance, None)?;
                    }
                }
            }
        }
        let length = sum[0].hypot(sum[1]);
        if length > limit {
            let scale = limit / length;
            sum = [sum[0] * scale, sum[1] * scale];
        }
        let next_pol = if let Some((pcp, polarities, _, _, gradients)) = pcp_inputs {
            let p_i = polarities[index];
            let p_i_perp = [-p_i[1], p_i[0]];
            let grad_q = gradients[index];
            let tau_cue =
                pcp.gradient_cue_strength * (p_i_perp[0] * grad_q[0] + p_i_perp[1] * grad_q[1]);
            let tau_i = tau_cue + tau_neighbors;
            let dtheta = pcp.dt * pcp.torque_mobility * tau_i;
            rotate_unit_polarity(p_i, dtheta)?
        } else {
            current_pol
        };
        let next_notch = if notch_h0.is_some() {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "kin-weighted neighbor average in [-1, 1] stored as f32 cell state"
            )]
            let value = ((notch_num / notch_den.max(1.0e-6)) as f32).clamp(-1.0_f32, 1.0_f32);
            if value == 0.0 { 0.0 } else { value }
        } else {
            0.0_f32
        };
        let next_vel = if let Some((va_cfg, _)) = velocity_inputs {
            align_velocity_dissipative(current_vel, align_v, align_w, va_cfg.strength)
        } else {
            current_vel
        };
        Ok((
            sum,
            overlapping,
            next_pol,
            next_notch,
            delta_k,
            TrophallaxisSummary {
                total_out_quanta: body_out,
                total_in_quanta: body_in,
                total_tax_quanta: body_tax,
            },
            next_vel,
        ))
    };

    let accumulate_summary =
        |acc: &mut TrophallaxisSummary, add: TrophallaxisSummary| -> Result<(), CoreError> {
            acc.total_out_quanta = acc
                .total_out_quanta
                .checked_add(add.total_out_quanta)
                .ok_or(CoreError::Overflow {
                    quantity: "trophallaxis total outgoing quanta",
                })?;
            acc.total_in_quanta = acc.total_in_quanta.checked_add(add.total_in_quanta).ok_or(
                CoreError::Overflow {
                    quantity: "trophallaxis total incoming quanta",
                },
            )?;
            acc.total_tax_quanta = acc
                .total_tax_quanta
                .checked_add(add.total_tax_quanta)
                .ok_or(CoreError::Overflow {
                    quantity: "trophallaxis total tax quanta",
                })?;
            Ok(())
        };

    let fill = |first: usize,
                out: &mut [[f64; 2]],
                pushes: &mut [[f64; 2]],
                pols: &mut [[f64; 2]],
                notches: &mut [f32],
                deltas: &mut [i64],
                vels: &mut [[f64; 2]]|
     -> Result<TrophallaxisSummary, CoreError> {
        let mut chunk_summary = TrophallaxisSummary::default();
        for (offset, slot) in out.iter_mut().enumerate() {
            let (displacement, push, next_pol, next_notch, delta_k, body_tr, next_vel) =
                eval_body(first + offset)?;
            *slot = displacement;
            if let Some(push_slot) = pushes.get_mut(offset) {
                *push_slot = push;
            }
            if let Some(pol_slot) = pols.get_mut(offset) {
                *pol_slot = next_pol;
            }
            if let Some(notch_slot) = notches.get_mut(offset) {
                *notch_slot = next_notch;
            }
            if let Some(delta_slot) = deltas.get_mut(offset) {
                *delta_slot = delta_k;
            }
            if let Some(vel_slot) = vels.get_mut(offset) {
                *vel_slot = next_vel;
            }
            accumulate_summary(&mut chunk_summary, body_tr)?;
        }
        Ok(chunk_summary)
    };

    let pol_slice: &mut [[f64; 2]] = next_polarities.map_or(&mut [], Vec::as_mut_slice);
    let notch_slice: &mut [f32] = neighbor_h0.map_or(&mut [], Vec::as_mut_slice);
    let delta_slice: &mut [i64] = trophallaxis_delta_quanta.map_or(&mut [], Vec::as_mut_slice);
    let vel_slice: &mut [[f64; 2]] = next_velocities.map_or(&mut [], Vec::as_mut_slice);
    let total_summary = if used == 1 {
        fill(
            0,
            displacements,
            pressed,
            pol_slice,
            notch_slice,
            delta_slice,
            vel_slice,
        )?
    } else {
        let length = count.div_ceil(used);
        let mut chunks = displacements.chunks_mut(length);
        let mut push_chunks = pressed
            .chunks_mut(length)
            .chain(std::iter::repeat_with(|| -> &mut [[f64; 2]] { &mut [] }));
        let mut pol_chunks = pol_slice
            .chunks_mut(length)
            .chain(std::iter::repeat_with(|| -> &mut [[f64; 2]] { &mut [] }));
        let mut notch_chunks = notch_slice
            .chunks_mut(length)
            .chain(std::iter::repeat_with(|| -> &mut [f32] { &mut [] }));
        let mut delta_chunks = delta_slice
            .chunks_mut(length)
            .chain(std::iter::repeat_with(|| -> &mut [i64] { &mut [] }));
        let mut vel_chunks = vel_slice
            .chunks_mut(length)
            .chain(std::iter::repeat_with(|| -> &mut [[f64; 2]] { &mut [] }));
        let head = chunks.next().unwrap_or_default();
        let push_head = push_chunks.next().unwrap_or_default();
        let pol_head = pol_chunks.next().unwrap_or_default();
        let notch_head = notch_chunks.next().unwrap_or_default();
        let delta_head = delta_chunks.next().unwrap_or_default();
        let vel_head = vel_chunks.next().unwrap_or_default();
        let fill = &fill;
        let tail_count = count.div_ceil(length).saturating_sub(1);
        let mut tail_results: Vec<Result<TrophallaxisSummary, CoreError>> = (0..tail_count)
            .map(|_| Ok(TrophallaxisSummary::default()))
            .collect();
        let mut head_result = Ok(TrophallaxisSummary::default());
        flife_core::pool::scope_workers(workers, "contact", |scope| {
            for (index, ((((((chunk, pushes), pols), notches), deltas), vels), slot)) in chunks
                .zip(push_chunks)
                .zip(pol_chunks)
                .zip(notch_chunks)
                .zip(delta_chunks)
                .zip(vel_chunks)
                .zip(tail_results.iter_mut())
                .enumerate()
            {
                scope.spawn(move || {
                    *slot = fill(
                        (index + 1) * length,
                        chunk,
                        pushes,
                        pols,
                        notches,
                        deltas,
                        vels,
                    );
                });
            }
            head_result = fill(
                0, head, push_head, pol_head, notch_head, delta_head, vel_head,
            );
        })?;
        let mut acc = head_result?;
        for chunk in tail_results {
            accumulate_summary(&mut acc, chunk?)?;
        }
        acc
    };

    if let Some(summary_out) = trophallaxis_summary {
        let conserved = total_summary
            .total_in_quanta
            .checked_add(total_summary.total_tax_quanta);
        if conserved != Some(total_summary.total_out_quanta) {
            return Err(CoreError::StateConflict {
                operation: "trophallaxis",
                detail: format!(
                    "exact quantum conservation violated: out={} != in={} + tax={}",
                    total_summary.total_out_quanta,
                    total_summary.total_in_quanta,
                    total_summary.total_tax_quanta
                ),
            });
        }
        *summary_out = total_summary;
    }
    Ok(())
}

/// Reusable post-Phase-1 crowding buffers for `V2-07` (`hippo-crowding`).
///
/// Empty (0 bytes) when `hippo-crowding` is disabled or on `field-ecology-v1*`.
/// Pre-reserved to population capacity at founding so stepping never allocates
/// (`FL-043`, `INV-15`).
#[derive(Debug, Default)]
pub struct CrowdingWork {
    /// Post-Phase-1 local neighbor count `N_loc` per row in `rows` order.
    counts: Vec<u32>,
    /// Contiguous copy of post-Phase-1 wrapped positions (avoids per-tick alloc
    /// and improves spatial-scan cache locality).
    positions: Vec<[f64; 2]>,
    /// Spatial counting-sort buffers used only by `local-neighbors-v1`.
    bins: Vec<u32>,
    starts: Vec<u32>,
    members: Vec<u32>,
    /// Cell occupancy buffers used only by `cell-occupancy-v1`.
    cell_counts: Vec<u32>,
    cells: Vec<usize>,
}

impl CrowdingWork {
    /// Pre-allocate buffers for `local-neighbors-v1` over at most `rows`
    /// organisms at crowding `radius` on `grid`.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when a reservation fails, or
    /// [`CoreError::NumericalInvalid`] if `radius` is not finite and positive.
    pub fn with_capacity_local_neighbors(
        grid: &FieldGrid,
        radius: f64,
        rows: usize,
    ) -> Result<Self, CoreError> {
        if !radius.is_finite() || radius <= 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "v2.crowding.radius_world_units",
                detail: format!("{radius} must be finite and positive"),
            });
        }
        let bins = bins_along(grid.world_width(), radius) * bins_along(grid.world_height(), radius);
        let mut work = Self::default();
        reserve(&mut work.counts, rows, "crowding counts")?;
        reserve(&mut work.positions, rows, "crowding positions")?;
        reserve(&mut work.bins, rows, "crowding bins")?;
        reserve(&mut work.starts, bins + 1, "crowding bin starts")?;
        reserve(&mut work.members, rows, "crowding members")?;
        Ok(work)
    }

    /// Pre-allocate buffers for `cell-occupancy-v1` over at most `rows`
    /// organisms on `grid`.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when a reservation fails.
    pub fn with_capacity_cell_occupancy(grid: &FieldGrid, rows: usize) -> Result<Self, CoreError> {
        let cell_count = usize::try_from(grid.cell_count()).map_err(|_| CoreError::Overflow {
            quantity: "grid cells",
        })?;
        let mut work = Self::default();
        reserve(&mut work.counts, rows, "crowding counts")?;
        reserve(&mut work.positions, rows, "crowding positions")?;
        reserve(&mut work.cell_counts, cell_count, "cell occupancy")?;
        reserve(&mut work.cells, rows, "organism crowding cells")?;
        Ok(work)
    }

    /// Host bytes held by all crowding buffers (0 when disabled).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        [
            bytes(&self.counts),
            bytes(&self.positions),
            bytes(&self.bins),
            bytes(&self.starts),
            bytes(&self.members),
            bytes(&self.cell_counts),
            bytes(&self.cells),
        ]
        .into_iter()
        .fold(0_u64, u64::saturating_add)
    }

    /// Most recently computed per-row local crowding counts `N_loc`.
    #[must_use]
    pub fn counts(&self) -> &[u32] {
        &self.counts
    }

    /// Mutable access to the pre-reserved position scratch buffer.
    pub fn positions_mut(&mut self) -> &mut Vec<[f64; 2]> {
        &mut self.positions
    }

    /// Clear `counts` without releasing reserved capacity.
    pub fn clear(&mut self) {
        self.counts.clear();
        self.positions.clear();
    }

    /// Count `local-neighbors-v1` over the positions buffered in
    /// [`Self::positions_mut`].
    ///
    /// # Errors
    /// As [`Self::count_local_neighbors`].
    pub fn count_buffered_local_neighbors(
        &mut self,
        grid: &FieldGrid,
        radius: f64,
        workers: usize,
    ) -> Result<&[u32], CoreError> {
        let positions = core::mem::take(&mut self.positions);
        let outcome = self
            .count_local_neighbors(grid, radius, &positions, workers)
            .map(|_| ());
        self.positions = positions;
        outcome.map(|()| self.counts.as_slice())
    }

    /// Count `cell-occupancy-v1` over the positions buffered in
    /// [`Self::positions_mut`].
    ///
    /// # Errors
    /// As [`Self::count_cell_occupancy`].
    pub fn count_buffered_cell_occupancy(&mut self, grid: &FieldGrid) -> Result<&[u32], CoreError> {
        let positions = core::mem::take(&mut self.positions);
        let outcome = self.count_cell_occupancy(grid, &positions).map(|_| ());
        self.positions = positions;
        outcome.map(|()| self.counts.as_slice())
    }

    /// Count exact integer neighbors within `d_ij < radius` on the torus for
    /// each position in `positions` (`local-neighbors-v1`, `V2-07`), using up
    /// to `workers` threads.
    ///
    /// Integer counting is associative and commutative, so the output is
    /// bit-identical at every worker count (`INV-11`).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a non-positive/non-finite `radius`
    /// or an unwrapped position.
    pub fn count_local_neighbors(
        &mut self,
        grid: &FieldGrid,
        radius: f64,
        positions: &[[f64; 2]],
        workers: usize,
    ) -> Result<&[u32], CoreError> {
        if !radius.is_finite() || radius <= 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "v2.crowding.radius_world_units",
                detail: format!("{radius} must be finite and positive"),
            });
        }
        let count = positions.len();
        self.counts.clear();
        self.counts.resize(count, 0);
        if count == 0 {
            return Ok(&self.counts);
        }
        if count >= u32::MAX as usize {
            return Err(CoreError::Overflow {
                quantity: "crowding bodies",
            });
        }
        let (width, height) = (grid.world_width(), grid.world_height());
        let (bins_x, bins_y) = (bins_along(width, radius), bins_along(height, radius));
        let total_bins = bins_x * bins_y;
        self.starts.clear();
        self.starts.resize(total_bins + 1, 0);
        self.bins.clear();
        for &[x, y] in positions {
            if !(0.0..width).contains(&x) || !(0.0..height).contains(&y) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "crowding position",
                    detail: format!("[{x:?}, {y:?}] is not wrapped"),
                });
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "total_bins <= MAXIMUM_BINS_PER_SIDE^2 = 4_194_304 < u32::MAX"
            )]
            let bin = (bin_of(y, height, bins_y) * bins_x + bin_of(x, width, bins_x)) as u32;
            self.bins.push(bin);
            self.starts[bin as usize] += 1;
        }
        let mut running = 0_u32;
        for slot in &mut self.starts[..total_bins] {
            running += *slot;
            *slot = running;
        }
        self.starts[total_bins] = running;
        self.members.clear();
        self.members.resize(count, 0);
        for (index, &bin) in self.bins.iter().enumerate().rev() {
            let b = bin as usize;
            self.starts[b] -= 1;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "count < u32::MAX checked above"
            )]
            let body = index as u32;
            self.members[self.starts[b] as usize] = body;
        }

        let radius_sq = radius * radius;
        let use_sq_bounds = radius_sq.is_finite() && radius_sq > 0.0;
        let inside_sq = radius_sq * (1.0 - 1e-9);
        let beyond = radius_sq * (1.0 + 1e-9);
        let (half_width, half_height) = (0.5 * width, 0.5 * height);
        let (starts, members, bins) = (&*self.starts, &*self.members, &*self.bins);
        let neighbors_for = |index: usize| -> u32 {
            let [here_x, here_y] = positions[index];
            let bin = bins[index] as usize;
            let (bx, by) = (bin % bins_x, bin / bins_x);
            let (rows, row_count) = neighbours(by, bins_y);
            let (columns, column_count) = neighbours(bx, bins_x);
            let mut n = 0_u32;
            for &row in &rows[..row_count] {
                let row_base = row * bins_x;
                for &column in &columns[..column_count] {
                    let neighbour = row_base + column;
                    let lo = starts[neighbour] as usize;
                    let hi = starts[neighbour + 1] as usize;
                    for &other_u32 in &members[lo..hi] {
                        let other = other_u32 as usize;
                        if other == index {
                            continue;
                        }
                        let there = positions[other];
                        let dx = image(there[0], here_x, width, half_width);
                        if dx.abs() >= radius {
                            continue;
                        }
                        let dy = image(there[1], here_y, height, half_height);
                        if dy.abs() >= radius {
                            continue;
                        }
                        let within = if use_sq_bounds {
                            let dist_sq = dx * dx + dy * dy;
                            dist_sq < inside_sq || (dist_sq <= beyond && dx.hypot(dy) < radius)
                        } else {
                            dx.hypot(dy) < radius
                        };
                        if within {
                            n = n.saturating_add(1);
                        }
                    }
                }
            }
            n
        };
        let fill = |first: usize, out: &mut [u32]| {
            for (offset, slot) in out.iter_mut().enumerate() {
                *slot = neighbors_for(first + offset);
            }
        };
        let used = workers
            .max(1)
            .min(count.div_ceil(MINIMUM_BODIES_PER_WORKER))
            .max(1);
        if used == 1 {
            fill(0, &mut self.counts);
            return Ok(&self.counts);
        }
        let length = count.div_ceil(used);
        let mut chunks = self.counts.chunks_mut(length);
        let head = chunks.next().unwrap_or_default();
        let fill = &fill;
        flife_core::pool::scope_workers(workers, "crowding", |scope| {
            for (index, chunk) in chunks.enumerate() {
                scope.spawn(move || fill((index + 1) * length, chunk));
            }
            fill(0, head);
        })?;
        Ok(&self.counts)
    }

    /// Count pure-field grid-cell co-occupants `N_loc = n_c[cell] - 1` for
    /// each position in `positions` (`cell-occupancy-v1`, `V2-07`, `INV-08`).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for an unwrapped position.
    pub fn count_cell_occupancy(
        &mut self,
        grid: &FieldGrid,
        positions: &[[f64; 2]],
    ) -> Result<&[u32], CoreError> {
        let count = positions.len();
        self.counts.clear();
        self.counts.resize(count, 0);
        if count == 0 {
            return Ok(&self.counts);
        }
        let (width, height) = (grid.world_width(), grid.world_height());
        let (cells_x, cells_y) = (grid.cells_x() as usize, grid.cells_y() as usize);
        let cell_count = usize::try_from(grid.cell_count()).map_err(|_| CoreError::Overflow {
            quantity: "grid cells",
        })?;
        self.cell_counts.clear();
        self.cell_counts.resize(cell_count, 0);
        self.cells.clear();
        for &[x, y] in positions {
            if !(0.0..width).contains(&x) || !(0.0..height).contains(&y) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "crowding position",
                    detail: format!("[{x:?}, {y:?}] is not wrapped"),
                });
            }
            let cx = bin_of(x, width, cells_x);
            let cy = bin_of(y, height, cells_y);
            let cell = cy * cells_x + cx;
            self.cells.push(cell);
            self.cell_counts[cell] = self.cell_counts[cell].saturating_add(1);
        }
        for (slot, &cell) in self.counts.iter_mut().zip(&self.cells) {
            *slot = self.cell_counts[cell].saturating_sub(1);
        }
        Ok(&self.counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(side: f64, cells: u32) -> FieldGrid {
        FieldGrid::new(cells, cells, side, side).expect("a valid grid")
    }

    const PLAIN: ContactRules = ContactRules {
        diameter: 0.5,
        stiffness: 0.5,
        adhesion: 0.0,
        range: 1.0,
        sweeps: 1,
        inhibits: false,
        inhibition: 0.0,
        repulsion: 0.0,
    };

    fn run_with(
        grid: &FieldGrid,
        rules: &ContactRules,
        positions: &[[f64; 2]],
        affinities: Option<&[[f32; AFFINITY_TERMS]]>,
    ) -> Vec<[f64; 2]> {
        let ids: Vec<u64> = (0..positions.len() as u64).collect();
        let alive = vec![true; positions.len()];
        let mut out = Vec::new();
        relax(
            grid,
            rules,
            Bodies {
                positions,
                ids: &ids,
                alive: &alive,
                affinities,
            },
            &mut out,
            &mut ContactScratch::default(),
            1,
        )
        .expect("contact");
        out
    }

    fn run(grid: &FieldGrid, diameter: f64, positions: &[[f64; 2]]) -> Vec<[f64; 2]> {
        let rules = ContactRules { diameter, ..PLAIN };
        run_with(grid, &rules, positions, None)
    }

    #[test]
    fn two_overlapping_disks_separate_symmetrically() {
        let grid = grid(16.0, 16);
        let out = run(&grid, 0.5, &[[4.0, 4.0], [4.3, 4.0]]);
        // Overlap 0.2; each moves k/2 · 0.2 = 0.05 away from the other.
        assert!((out[0][0] + 0.05).abs() < 1e-15, "{out:?}");
        assert!((out[1][0] - 0.05).abs() < 1e-15, "{out:?}");
        assert_eq!(out[0][1], 0.0);
        assert_eq!(out[1][1], 0.0);
    }

    #[test]
    fn disks_apart_or_dead_do_not_push() {
        let grid = grid(16.0, 16);
        assert_eq!(
            run(&grid, 0.5, &[[4.0, 4.0], [4.5, 4.0]]),
            vec![[0.0, 0.0]; 2]
        );
        let mut out = Vec::new();
        relax(
            &grid,
            &PLAIN,
            Bodies {
                positions: &[[4.0, 4.0], [4.1, 4.0]],
                ids: &[0, 1],
                alive: &[true, false],
                affinities: None,
            },
            &mut out,
            &mut ContactScratch::default(),
            1,
        )
        .expect("contact");
        assert_eq!(out, vec![[0.0, 0.0]; 2]);
    }

    #[test]
    fn contact_wraps_across_the_torus_seam() {
        let grid = grid(16.0, 16);
        let out = run(&grid, 0.5, &[[0.1, 8.0], [15.8, 8.0]]);
        // Distance 0.3 across the seam: the first is pushed to +x.
        assert!((out[0][0] - 0.05).abs() < 1e-12, "{out:?}");
        assert!((out[1][0] + 0.05).abs() < 1e-12, "{out:?}");
    }

    #[test]
    fn coincident_disks_separate_by_identity_and_the_push_is_clamped() {
        let grid = grid(16.0, 16);
        let out = run(&grid, 0.5, &[[3.0, 3.0], [3.0, 3.0]]);
        assert_eq!(out, vec![[-0.125, 0.0], [0.125, 0.0]]);
        // Many coincident neighbours: clamped to D/2 at most.
        let crowd = vec![[3.0, 3.0]; 9];
        for [dx, dy] in run(&grid, 0.5, &crowd) {
            assert!(dx.hypot(dy) <= 0.25 + 1e-15);
        }
    }

    #[test]
    fn small_tori_find_each_neighbour_once() {
        // Two bins a side: each pair must be counted once, not twice.
        let two = grid(1.2, 1);
        let out = run(&two, 0.5, &[[0.1, 0.1], [0.4, 0.1]]);
        assert!((out[0][0] + 0.05).abs() < 1e-12, "{out:?}");
        let one = grid(0.6, 1);
        let out = run(&one, 0.5, &[[0.1, 0.1], [0.4, 0.1]]);
        // One bin a side, distance 0.3 directly (the image is 0.3 too).
        assert!(out[0][0].abs() <= 0.05 + 1e-12, "{out:?}");
    }

    fn axis(channel: usize) -> [f32; AFFINITY_TERMS] {
        let mut out = [0.0; AFFINITY_TERMS];
        out[channel] = 1.0;
        out
    }

    #[test]
    fn like_bodies_near_touching_pull_together_and_unlike_ones_do_not() {
        let grid = grid(16.0, 16);
        let rules = ContactRules {
            diameter: 0.5,
            stiffness: 0.5,
            adhesion: 0.4,
            range: 1.6,
            ..PLAIN
        };
        // 0.65 apart: inside reach 0.8. Pull = (0.4/2) · 1 · 0.15 · 0.15 / 0.3.
        let positions = [[4.0, 4.0], [4.65, 4.0]];
        let pull = 0.2 * 0.15 * 0.15 / 0.3;
        let kin = run_with(&grid, &rules, &positions, Some(&[axis(0), axis(0)]));
        assert!((kin[0][0] - pull).abs() < 1e-12, "{kin:?}");
        assert!((kin[1][0] + pull).abs() < 1e-12, "{kin:?}");
        // Orthogonal chemistry: no pull; opposite chemistry: none either.
        let strangers = run_with(&grid, &rules, &positions, Some(&[axis(0), axis(1)]));
        assert_eq!(strangers, vec![[0.0, 0.0]; 2]);
        let mut opposite = axis(0);
        opposite[0] = -1.0;
        let enemies = run_with(&grid, &rules, &positions, Some(&[axis(0), opposite]));
        assert_eq!(enemies, vec![[0.0, 0.0]; 2]);
        // Beyond reach, nothing; adhesion needs its affinities.
        let far = run_with(
            &grid,
            &rules,
            &[[4.0, 4.0], [4.85, 4.0]],
            Some(&[axis(0); 2]),
        );
        assert_eq!(far, vec![[0.0, 0.0]; 2]);
        let mut out = Vec::new();
        assert!(
            relax(
                &grid,
                &rules,
                Bodies {
                    positions: &positions,
                    ids: &[0, 1],
                    alive: &[true, true],
                    affinities: None,
                },
                &mut out,
                &mut ContactScratch::default(),
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn repulsion_pushes_unlike_bodies_apart_over_adhesions_shell() {
        let grid = grid(16.0, 16);
        let sticky = ContactRules {
            diameter: 0.5,
            stiffness: 0.5,
            adhesion: 0.4,
            range: 1.6,
            ..PLAIN
        };
        // As in the test above: 0.65 apart, a full pull of 0.2 · 0.15² / 0.3.
        let positions = [[4.0, 4.0], [4.65, 4.0]];
        let pull = 0.2 * 0.15 * 0.15 / 0.3;
        let at = |repulsion: f64, b: [f32; AFFINITY_TERMS]| {
            let rules = ContactRules {
                repulsion,
                ..sticky
            };
            run_with(&grid, &rules, &positions, Some(&[axis(0), b]))
        };
        // No repulsion is the plain rule, bit for bit.
        let mut half = axis(0);
        half[1] = 1.0;
        let half = unit_affinity(&half.map(f64::from));
        for b in [axis(0), axis(1), half] {
            assert_eq!(
                at(0.0, b),
                run_with(&grid, &sticky, &positions, Some(&[axis(0), b]))
            );
        }
        // Identical chemistry pulls as before: (1 + r) · 1 − r = 1.
        let kin = at(1.0, axis(0));
        assert!((kin[0][0] - pull).abs() < 1e-12, "{kin:?}");
        // Orthogonal chemistry pushes apart, r times the full pull.
        for repulsion in [0.5, 1.0] {
            let strangers = at(repulsion, axis(1));
            assert!(
                (strangers[0][0] + repulsion * pull).abs() < 1e-12,
                "{strangers:?}"
            );
            assert!(
                (strangers[1][0] - repulsion * pull).abs() < 1e-12,
                "{strangers:?}"
            );
            assert_eq!(strangers[0][1], 0.0);
        }
        // cos² = ½ is the balance at r = 1: (1 + 1) · ½ − 1 = 0.
        let balanced = at(1.0, half);
        assert!(balanced[0][0].abs() < 1e-7, "{balanced:?}");
        // Beyond reach, nothing, however unlike.
        let rules = ContactRules {
            repulsion: 1.0,
            ..sticky
        };
        let far = run_with(
            &grid,
            &rules,
            &[[4.0, 4.0], [4.85, 4.0]],
            Some(&[axis(0), axis(1)]),
        );
        assert_eq!(far, vec![[0.0, 0.0]; 2]);
        // r is a share of the pull, in [0, 1].
        for repulsion in [-0.1, 1.5, f64::NAN] {
            let rules = ContactRules {
                repulsion,
                ..sticky
            };
            assert!(rules.check().is_err(), "{repulsion}");
        }
    }

    #[test]
    fn the_sum_matches_an_all_pairs_oracle() {
        let grid = grid(8.0, 8);
        let mut positions = Vec::new();
        let mut affinities = Vec::new();
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1_u64 << 53) as f64
        };
        for _ in 0..400 {
            positions.push([next() * 8.0, next() * 8.0]);
            let mut affinity = [0.0_f64; AFFINITY_TERMS];
            // Four families of chemistry, each a little varied.
            let family = (next() * 4.0) as usize;
            affinity[family] = 1.0;
            for value in &mut affinity {
                *value += 0.2 * (next() - 0.5);
            }
            affinities.push(unit_affinity(&affinity));
        }
        let rules = ContactRules {
            diameter: 0.45,
            stiffness: 0.6,
            adhesion: 0.3,
            range: 1.5,
            ..PLAIN
        };
        let out = run_with(&grid, &rules, &positions, Some(&affinities));
        let reach = 0.45 * 1.5;
        for (index, &[x, y]) in positions.iter().enumerate() {
            let mut sum = [0.0, 0.0];
            for (other, &position) in positions.iter().enumerate() {
                if other == index {
                    continue;
                }
                let [dx, dy] = grid.displacement(position, [x, y]).expect("wrapped");
                let distance = dx.hypot(dy);
                let push = if distance < 0.45 {
                    0.3 * (0.45 - distance)
                } else if distance < reach {
                    let alike = cosine(&affinities[index], &affinities[other]).max(0.0);
                    -0.15 * alike * alike * (distance - 0.45) * (reach - distance) / (0.5 * 0.45)
                } else {
                    continue;
                };
                if distance > 0.0 {
                    sum[0] += push * dx / distance;
                    sum[1] += push * dy / distance;
                }
            }
            let length = sum[0].hypot(sum[1]);
            if length > 0.225 {
                sum = [sum[0] * 0.225 / length, sum[1] * 0.225 / length];
            }
            assert!((out[index][0] - sum[0]).abs() < 1e-12, "{index}");
            assert!((out[index][1] - sum[1]).abs() < 1e-12, "{index}");
        }
    }

    #[test]
    fn the_displacement_is_the_same_at_any_worker_count() {
        let grid = grid(64.0, 64);
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1_u64 << 53) as f64
        };
        let count = 3 * MINIMUM_BODIES_PER_WORKER + 17;
        let positions: Vec<[f64; 2]> = (0..count).map(|_| [next() * 64.0, next() * 64.0]).collect();
        let affinities: Vec<[f32; AFFINITY_TERMS]> = (0..count)
            .map(|_| unit_affinity(&std::array::from_fn(|_| next() - 0.5)))
            .collect();
        let ids: Vec<u64> = (0..count as u64).collect();
        let alive: Vec<bool> = (0..count).map(|index| index % 11 != 3).collect();
        let rules = ContactRules {
            diameter: 0.5,
            stiffness: 0.5,
            adhesion: 1.0,
            range: 1.5,
            ..PLAIN
        };
        let at = |workers: usize| {
            let mut out = Vec::new();
            relax(
                &grid,
                &rules,
                Bodies {
                    positions: &positions,
                    ids: &ids,
                    alive: &alive,
                    affinities: Some(&affinities),
                },
                &mut out,
                &mut ContactScratch::default(),
                workers,
            )
            .expect("contact");
            out.iter()
                .flat_map(|d| d.map(f64::to_bits))
                .collect::<Vec<u64>>()
        };
        let one = at(1);
        assert!(one.iter().any(|&bits| bits != 0));
        for workers in [2, 3, 4, 16] {
            assert_eq!(at(workers), one, "{workers} workers");
        }
    }

    /// A world's contact work for `positions` (identity order, all alive),
    /// relaxed under `rules` on `workers`.
    fn work_with(
        grid: &FieldGrid,
        rules: &ContactRules,
        positions: &[[f64; 2]],
        affinities: Option<&[[f32; AFFINITY_TERMS]]>,
        workers: usize,
    ) -> ContactWork {
        let mut work =
            ContactWork::with_capacity(grid, rules, positions.len()).expect("contact buffers");
        let reserved = work.allocated_bytes();
        work.positions.extend_from_slice(positions);
        work.ids.extend(0..positions.len() as u64);
        work.alive
            .extend(std::iter::repeat_n(true, positions.len()));
        if let Some(affinities) = affinities {
            work.affinities.extend_from_slice(affinities);
        }
        work.relax(grid, rules, workers).expect("contact");
        assert_eq!(work.allocated_bytes(), reserved, "no sweep allocates");
        work
    }

    /// The deepest overlap `max (D − d)₊` among `positions` moved by
    /// `displacements` on a torus of `side`.
    fn deepest(
        positions: &[[f64; 2]],
        displacements: &[[f64; 2]],
        side: f64,
        diameter: f64,
    ) -> f64 {
        let moved: Vec<[f64; 2]> = positions
            .iter()
            .zip(displacements)
            .map(|(p, d)| {
                [
                    (p[0] + d[0]).rem_euclid(side),
                    (p[1] + d[1]).rem_euclid(side),
                ]
            })
            .collect();
        let mut deepest = 0.0_f64;
        for (index, a) in moved.iter().enumerate() {
            for b in &moved[index + 1..] {
                let dx = image(b[0], a[0], side, 0.5 * side);
                let dy = image(b[1], a[1], side, 0.5 * side);
                deepest = deepest.max(diameter - dx.hypot(dy));
            }
        }
        deepest
    }

    /// `count` bodies scattered in a disk of radius `radius` about (8, 8).
    fn pile(count: usize, radius: f64) -> Vec<[f64; 2]> {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1_u64 << 53) as f64
        };
        (0..count)
            .map(|_| {
                let (r, a) = (radius * next().sqrt(), std::f64::consts::TAU * next());
                [8.0 + r * a.cos(), 8.0 + r * a.sin()]
            })
            .collect()
    }

    #[test]
    fn one_sweep_is_the_plain_rule_and_more_sweeps_resolve_more_overlap() {
        let grid = grid(16.0, 16);
        let positions = pile(60, 1.5);
        let plain = run_with(&grid, &PLAIN, &positions, None);
        let once = work_with(&grid, &PLAIN, &positions, None, 1);
        assert_eq!(once.displacements, plain);
        assert_eq!(
            once.positions, positions,
            "one sweep leaves the gathered positions"
        );
        assert!(once.pressed.is_empty(), "the plain rule keeps no push");
        let mut last = deepest(&positions, &plain, 16.0, 0.5);
        for sweeps in [2, 4, 8] {
            let rules = ContactRules { sweeps, ..PLAIN };
            let work = work_with(&grid, &rules, &positions, None, 1);
            let now = deepest(&positions, &work.displacements, 16.0, 0.5);
            assert!(
                now < last,
                "{sweeps} sweeps: deepest overlap {now}, fewer: {last}"
            );
            last = now;
            // The positions a sweep leaves are the gathered ones moved by the
            // summed displacement, wrapped.
            for ((p, d), q) in positions
                .iter()
                .zip(&work.displacements)
                .zip(&work.positions)
            {
                let wrapped = grid
                    .wrap_position([p[0] + d[0], p[1] + d[1]])
                    .expect("wrap");
                assert_eq!(&wrapped, q);
            }
        }
    }

    #[test]
    fn the_push_counts_overlaps_and_not_adhesion() {
        let grid = grid(16.0, 16);
        let rules = ContactRules {
            adhesion: 1.0,
            range: 1.5,
            inhibits: true,
            ..PLAIN
        };
        // Body 0 overlaps body 1 (0.3 apart) and is pulled by body 2 (0.65
        // apart, inside reach 0.75); all alike.
        let positions = [[4.0, 4.0], [4.3, 4.0], [4.0, 4.65]];
        let alike = [axis(0), axis(0), axis(0)];
        let work = work_with(&grid, &rules, &positions, Some(&alike), 1);
        let push = 0.25 * (0.5 - 0.3);
        assert!(
            (work.pressed[0][0] + push).abs() < 1e-15,
            "{:?}",
            work.pressed[0]
        );
        assert_eq!(work.pressed[0][1], 0.0, "adhesion's pull is not a push");
        assert!(work.displacements[0][1] > 0.0, "but it moves the body");
        assert_eq!(work.pressed[2], [0.0, 0.0], "body 2 overlaps nothing");
        // Summed over sweeps.
        let swept = work_with(
            &grid,
            &ContactRules { sweeps: 3, ..rules },
            &positions,
            Some(&alike),
            1,
        );
        assert!(swept.pressed[0][0] < work.pressed[0][0]);
    }

    #[test]
    fn sweeps_and_pushes_are_the_same_at_any_worker_count() {
        let grid = grid(64.0, 64);
        let positions: Vec<[f64; 2]> = pile(20_000, 30.0)
            .into_iter()
            .map(|p| p.map(|x| x.rem_euclid(64.0)))
            .collect();
        let rules = ContactRules {
            adhesion: 1.0,
            range: 1.5,
            sweeps: 4,
            inhibits: true,
            ..PLAIN
        };
        let affinities: Vec<[f32; AFFINITY_TERMS]> =
            (0..positions.len()).map(|index| axis(index % 3)).collect();
        let bits = |workers: usize| {
            let work = work_with(&grid, &rules, &positions, Some(&affinities), workers);
            work.displacements
                .iter()
                .chain(&work.pressed)
                .chain(&work.positions)
                .flat_map(|d| d.map(f64::to_bits))
                .collect::<Vec<u64>>()
        };
        let one = bits(1);
        for workers in [2, 3, 4, 16] {
            assert_eq!(bits(workers), one, "{workers} workers");
        }
    }

    #[test]
    fn trophallaxis_fused_neighbor_cache_matches_uncached_and_worker_counts_bit_for_bit() {
        let grid = grid(32.0, 32);
        let rules = ContactRules {
            diameter: 0.5,
            stiffness: 0.5,
            adhesion: 0.8,
            range: 2.0,
            sweeps: 1,
            inhibits: true,
            inhibition: 0.5,
            repulsion: 0.25,
        };
        let count = 512;
        let positions: Vec<[f64; 2]> = pile(count, 6.0)
            .into_iter()
            .map(|p| [p[0].rem_euclid(32.0), p[1].rem_euclid(32.0)])
            .collect();
        let ids: Vec<u64> = (1..=count as u64).collect();
        let alive = vec![true; count];
        let affinities: Vec<[f32; AFFINITY_TERMS]> =
            (0..count).map(|i| axis(i % 4)).collect();
        let energies: Vec<f64> = (0..count)
            .map(|i| 0.5 + 3.0 * ((i * 37) % 101) as f64 / 100.0)
            .collect();
        let tr_params = TrophallaxisSweepParams {
            dt: 0.05,
            conductance_per_second: 1.5,
            transfer_efficiency: 0.9,
            reserve_energy: 1.0,
            maximum_export_fraction: 0.25,
            kin_cosine_exponent: 4,
            kin_threshold: 0.5,
        };
        let bodies = Bodies {
            positions: &positions,
            ids: &ids,
            alive: &alive,
            affinities: Some(&affinities),
        };
        let context = Sweep1Context {
            trophallaxis: Some(tr_params),
            energies: Some(&energies),
            ..Sweep1Context::default()
        };

        // Baseline: uncached (ContactScratch::default() has 0 neighbor_cache capacity).
        let mut uncached_disp = Vec::new();
        let mut uncached_deltas = Vec::new();
        let mut uncached_summary = TrophallaxisSummary::default();
        let mut uncached_scratch = ContactScratch::default();
        relax_sweep1_with_context(
            &grid,
            &rules,
            bodies,
            &context,
            Sweep1Output {
                displacements: &mut uncached_disp,
                next_polarities: None,
                neighbor_h0: None,
                trophallaxis_delta_quanta: Some(&mut uncached_deltas),
                trophallaxis_summary: Some(&mut uncached_summary),
                next_velocities: None,
            },
            &mut uncached_scratch,
            1,
        )
        .expect("uncached sweep1");
        assert!(
            uncached_summary.total_out_quanta > 0,
            "fixture must exercise live trophallaxis transfers"
        );
        assert_eq!(
            uncached_summary.total_out_quanta,
            uncached_summary.total_in_quanta + uncached_summary.total_tax_quanta
        );

        // Cached (ContactWork pre-reserves neighbor_cache) across workers 1, 2, 4,
        // plus tiny forced-overflow cache (capacity = 32 entries).
        for (label, workers, tiny_cap) in [
            ("cached w=1", 1, None),
            ("cached w=2", 2, None),
            ("cached w=4", 4, None),
            ("overflow w=1", 1, Some(32)),
            ("overflow w=4", 4, Some(32)),
        ] {
            let mut work = ContactWork::with_capacity_v2_features(
                &grid,
                &rules,
                count,
                ContactV2Buffers {
                    trophallaxis: true,
                    ..ContactV2Buffers::default()
                },
            )
            .expect("contact work");
            if let Some(cap) = tiny_cap {
                work.scratch.neighbor_cache = Vec::with_capacity(cap);
            }
            let bytes_before = work.allocated_bytes();
            work.positions.extend_from_slice(&positions);
            work.ids.extend_from_slice(&ids);
            work.alive.extend_from_slice(&alive);
            work.affinities.extend_from_slice(&affinities);
            work.energies.extend_from_slice(&energies);
            work.relax_v2_features(
                &grid,
                &rules,
                ContactV2Sweep {
                    trophallaxis: Some(tr_params),
                    ..ContactV2Sweep::default()
                },
                workers,
            )
            .expect(label);
            assert_eq!(work.allocated_bytes(), bytes_before, "{label}: allocated");
            assert_eq!(work.trophallaxis_summary, uncached_summary, "{label}: summary");
            assert_eq!(
                work.trophallaxis_delta_quanta, uncached_deltas,
                "{label}: deltas"
            );
            for (i, (got, want)) in work
                .displacements
                .iter()
                .zip(&uncached_disp)
                .enumerate()
            {
                assert_eq!(
                    (got[0].to_bits(), got[1].to_bits()),
                    (want[0].to_bits(), want[1].to_bits()),
                    "{label}: body {i}"
                );
            }
        }
    }

    #[test]
    fn crowding_local_neighbors_u32_bins_and_boundary_band_match_exact_hypot() {
        let grid = grid(16.0, 16);
        let radius = 1.25_f64;
        // Include pairs well inside, right around radius*(1 +/- 1e-10), and outside.
        let mut positions = vec![
            [8.0, 8.0],
            [8.0 + radius * (1.0 - 1e-10), 8.0],
            [8.0 + radius * (1.0 + 1e-10), 8.0],
            [8.0, 8.0 + radius * (1.0 - 1e-12)],
            [8.0, 8.0 + radius],
            [8.0 + 0.5, 8.0 + 0.5],
        ];
        positions.extend(
            pile(120, 4.0)
                .into_iter()
                .map(|p| [p[0].rem_euclid(16.0), p[1].rem_euclid(16.0)]),
        );
        let mut expected = vec![0_u32; positions.len()];
        for (i, &[xi, yi]) in positions.iter().enumerate() {
            for (j, &[xj, yj]) in positions.iter().enumerate() {
                if i == j {
                    continue;
                }
                let dx = image(xj, xi, 16.0, 8.0);
                let dy = image(yj, yi, 16.0, 8.0);
                if dx.hypot(dy) < radius {
                    expected[i] += 1;
                }
            }
        }
        let mut work =
            CrowdingWork::with_capacity_local_neighbors(&grid, radius, positions.len()).expect("cw");
        let bytes_before = work.allocated_bytes();
        for workers in [1, 4] {
            let got = work
                .count_local_neighbors(&grid, radius, &positions, workers)
                .expect("count");
            assert_eq!(got, expected.as_slice(), "workers={workers}");
            assert_eq!(work.allocated_bytes(), bytes_before, "workers={workers}");
        }
    }
}
