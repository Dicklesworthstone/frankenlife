//! `field-ecology-v2` morphogenesis geometry and `so2-motor` neural-agency
//! diagnostics (`V2-10` / `V2-17`).
//!
//! This module computes two families of pure, read-only diagnostics directly
//! from committed simulation state (a live [`World`], [`OrganismSnapshot`]s, a
//! restored [`Inspection`], or raw coordinate/telemetry slices) without
//! altering scientific state (`INV-09`) or existing CLI JSON schemas:
//!
//! 1. **Periodic tissue geometry ([`GeometryDiagnostics`]):**
//!    - `median_d1_over_D`: exact periodic median nearest-neighbor separation
//!      normalized by body diameter `D`;
//!    - `mean_overlaps_within_1D`: mean count of neighbors with `d < D`;
//!    - `coord_hist_0_to_8`: normalized coordination histogram for `z = 0..=8`
//!      (clamped at `8`) within `r_coord = 1.3 D`;
//!    - `chain_fraction_z1_z2`: `P(z = 1) + P(z = 2)`;
//!    - `mean_abs_psi6` and `global_abs_mean_psi6`: local and global hexatic
//!      bond-orientational order using denominator `max(z_i, 6)` so 1-cell
//!      chains and hollow rings (`z = 2`) cannot masquerade as `z = 6`
//!      hexagonal crystals. Bond angles use host-independent algebraic complex
//!      powers `(u_x + i u_y)^6` rather than platform `libm` trigonometry.
//! 2. **Per-organism `so2-motor` agency correlations ([`MotorAgencyDiagnostics`]):**
//!    - `corr(m_e, dE/dt)` and `corr(m_e, E)`, where `m_e = tanh(o_e)` is the
//!      decoded energy-gradient motor modulation (`0.0` under `chemistry-only`
//!      controls, yielding zero variance and `None` correlations).

use flife_core::model::ACTION_WIDTH;
use flife_core::{CoreError, V2Feature, numerics};
use flife_engine::{ModelVariant, OrganismSnapshot, World};
use serde::{Deserialize, Serialize};

use crate::inspect::Inspection;

/// Coordination shell multiplier `r_coord / D` used for `z`, `chain_fraction_z1_z2`,
/// and hexatic order `ψ_6` (`V2-10` / `V2-17`).
pub const COORDINATION_RADIUS_MULTIPLIER: f64 = 1.3;

/// Minimum denominator in `ψ_6(i) = (Σ_{j ∈ N_i} e^{i 6 θ_{ij}}) / max(z_i, 6)`
/// so low-coordination chains/rings (`z = 1, 2`) cannot saturate to `|ψ_6| = 1`.
pub const PSI6_MIN_DENOMINATOR: usize = 6;

/// Index of `so2-motor`'s `o_e` logit inside held controller action outputs
/// (`flife_neural::ActionLayout` places `so2-motor` immediately after the 36
/// baseline outputs).
pub const SO2_MOTOR_ACTION_OFFSET: usize = ACTION_WIDTH as usize;

/// Maximum side length of the temporary periodic spatial hash grid, bounding
/// scratch memory while preserving `cell_size >= r_coord` whenever `span >= r_coord`.
const MAX_GRID_SIDE: usize = 1024;

/// Periodic tissue and morphogenesis geometry diagnostics (`V2-10` / `V2-17`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeometryDiagnostics {
    /// Number of organisms included in the measurement.
    pub count: usize,
    /// Body diameter `D` used to normalize distances, overlaps, and `r_coord = 1.3 D`.
    pub diameter: f64,
    /// Median nearest-neighbor distance divided by `D`; `None` when `count < 2`.
    #[serde(rename = "median_d1_over_D")]
    pub median_d1_over_d: Option<f64>,
    /// Mean count of neighbors strictly inside `d < D` per organism (`0.0` when empty).
    #[serde(rename = "mean_overlaps_within_1D")]
    pub mean_overlaps_within_1d: f64,
    /// Normalized coordination histogram for `z = 0..=8` (clamped at `8`) within
    /// `r_coord = 1.3 D`; entries sum to `1.0` when `count > 0`, all `0.0` when empty.
    pub coord_hist_0_to_8: [f64; 9],
    /// Chain / filament fraction `P(z = 1) + P(z = 2)` within `r_coord = 1.3 D`.
    pub chain_fraction_z1_z2: f64,
    /// Local hexatic order `mean_i |ψ_6(i)|` with `max(z_i, 6)` denominator.
    pub mean_abs_psi6: f64,
    /// Global hexatic order `|mean_i ψ_6(i)|` with `max(z_i, 6)` denominator.
    pub global_abs_mean_psi6: f64,
}

/// One organism's decoded `so2-motor` energy-gradient steering and energy
/// telemetry for `V2-10` / `H2` agency diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MotorSample {
    /// Decoded energy-gradient motor modulation `m_e = tanh(o_e) ∈ [-1, 1]`.
    pub m_e: f64,
    /// Organism's committed internal energy `E`.
    pub energy: f64,
    /// Organism's committed energy change rate `dE/dt`, when available
    /// (`Some` for live [`World`] and [`OrganismSnapshot`]; `None` for
    /// checkpoint-only [`Inspection`]).
    pub energy_change_rate: Option<f64>,
}

/// Per-organism `so2-motor` (`H2`) neural-agency correlation diagnostics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MotorAgencyDiagnostics {
    /// Whether `so2-motor` is active on the inspected source.
    pub so2_motor_enabled: bool,
    /// Total live organisms inspected.
    pub total_count: usize,
    /// Live organisms with a valid held controller output containing `m_e`.
    pub evaluated_count: usize,
    /// Mean of decoded `m_e` across evaluated organisms; `None` when none evaluated.
    pub mean_me: Option<f64>,
    /// Population standard deviation of decoded `m_e`; `None` when fewer than 2 evaluated.
    pub std_me: Option<f64>,
    /// Pearson `corr(m_e, dE/dt)` across evaluated organisms with `dE/dt`
    /// available; `None` when fewer than 2 valid samples exist, `dE/dt` is
    /// unavailable, or either series has zero variance.
    pub corr_me_dedt: Option<f64>,
    /// Pearson `corr(m_e, E)` across evaluated organisms; `None` when fewer
    /// than 2 valid samples exist or either series has zero variance.
    pub corr_me_energy: Option<f64>,
}

impl MotorAgencyDiagnostics {
    /// Diagnostics record for a world or snapshot without `so2-motor`.
    #[must_use]
    pub const fn unavailable(total_count: usize) -> Self {
        Self {
            so2_motor_enabled: false,
            total_count,
            evaluated_count: 0,
            mean_me: None,
            std_me: None,
            corr_me_dedt: None,
            corr_me_energy: None,
        }
    }
}

/// Combined `V2-10` / `V2-17` geometry and neural-agency diagnostics for one
/// committed world or checkpoint inspection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V2Diagnostics {
    /// Periodic tissue geometry diagnostics; `None` when no body diameter is
    /// configured or supplied.
    pub geometry: Option<GeometryDiagnostics>,
    /// Per-organism `so2-motor` neural-agency correlation diagnostics.
    pub motor_agency: MotorAgencyDiagnostics,
}

/// Two-pass Pearson correlation coefficient `corr(xs, ys)` in `[-1, 1]`.
///
/// Returns `None` when `xs.len() != ys.len()`, fewer than 2 samples are given,
/// any input is nonfinite, or either series has zero variance.
#[must_use]
pub fn pearson_corr(xs: &[f64], ys: &[f64]) -> Option<f64> {
    if xs.len() != ys.len() || xs.len() < 2 {
        return None;
    }
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    for (&x, &y) in xs.iter().zip(ys.iter()) {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        sum_x += x;
        sum_y += y;
    }
    if !sum_x.is_finite() || !sum_y.is_finite() {
        return None;
    }
    let n = xs.len() as f64;
    let mean_x = sum_x / n;
    let mean_y = sum_y / n;
    let mut sxx = 0.0;
    let mut syy = 0.0;
    let mut sxy = 0.0;
    for (&x, &y) in xs.iter().zip(ys.iter()) {
        let dx = x - mean_x;
        let dy = y - mean_y;
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    if !sxx.is_finite() || !syy.is_finite() || !sxy.is_finite() || sxx <= 0.0 || syy <= 0.0 {
        return None;
    }
    let denom = sxx.sqrt() * syy.sqrt();
    if !denom.is_finite() || denom <= 0.0 {
        return None;
    }
    Some((sxy / denom).clamp(-1.0, 1.0))
}

/// Compute periodic tissue and hexatic geometry diagnostics for `positions` on
/// a torus of `extent = [width, height]` using body diameter `diameter`.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] if `extent` or `diameter` is nonfinite or
/// non-positive, if `r_coord²` overflows `f64`, or if any coordinate is
/// nonfinite.
pub fn geometry(
    positions: &[[f64; 2]],
    extent: [f64; 2],
    diameter: f64,
) -> Result<GeometryDiagnostics, CoreError> {
    let [width, height] = extent;
    if !width.is_finite() || width <= 0.0 || !height.is_finite() || height <= 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "world extent",
            detail: format!("extent [{width}, {height}] must be finite and positive"),
        });
    }
    if !diameter.is_finite() || diameter <= 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "body diameter",
            detail: format!("diameter {diameter} must be finite and positive"),
        });
    }
    let r_coord = COORDINATION_RADIUS_MULTIPLIER * diameter;
    let r_coord_sq = r_coord * r_coord;
    let r_overlap_sq = diameter * diameter;
    if !r_coord.is_finite()
        || !r_coord_sq.is_finite()
        || !r_overlap_sq.is_finite()
        || r_overlap_sq <= 0.0
        || r_coord_sq <= 0.0
    {
        return Err(CoreError::NumericalInvalid {
            quantity: "body diameter",
            detail: format!("diameter {diameter} overflows or underflows squared radius"),
        });
    }
    let half_w = 0.5 * width;
    let half_h = 0.5 * height;
    if !(half_w * half_w + half_h * half_h).is_finite() {
        return Err(CoreError::NumericalInvalid {
            quantity: "world extent",
            detail: format!("extent [{width}, {height}] overflows squared periodic distance"),
        });
    }

    let n = positions.len();
    let mut wrapped = Vec::with_capacity(n);
    for (index, &[x, y]) in positions.iter().enumerate() {
        if !x.is_finite() || !y.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "organism position",
                detail: format!("position {index} is nonfinite: [{x}, {y}]"),
            });
        }
        wrapped.push([wrap_coord(x, width), wrap_coord(y, height)]);
    }

    if n == 0 {
        return Ok(GeometryDiagnostics {
            count: 0,
            diameter,
            median_d1_over_d: None,
            mean_overlaps_within_1d: 0.0,
            coord_hist_0_to_8: [0.0; 9],
            chain_fraction_z1_z2: 0.0,
            mean_abs_psi6: 0.0,
            global_abs_mean_psi6: 0.0,
        });
    }
    if n == 1 {
        let mut hist = [0.0; 9];
        hist[0] = 1.0;
        return Ok(GeometryDiagnostics {
            count: 1,
            diameter,
            median_d1_over_d: None,
            mean_overlaps_within_1d: 0.0,
            coord_hist_0_to_8: hist,
            chain_fraction_z1_z2: 0.0,
            mean_abs_psi6: 0.0,
            global_abs_mean_psi6: 0.0,
        });
    }

    let grid = PeriodicGrid::build(&wrapped, [width, height], r_coord);
    let cell_min = grid.cell_w.min(grid.cell_h);
    let cell_min_sq = cell_min * cell_min;
    let fully_covered = grid.gx <= 3 && grid.gy <= 3;

    let mut d1_over_d = Vec::with_capacity(n);
    let mut overlap_sum: u64 = 0;
    let mut coord_counts = [0_u64; 9];
    let mut sum_abs_psi6 = 0.0;
    let mut global_psi6_re = 0.0;
    let mut global_psi6_im = 0.0;

    for (i, &[xi, yi]) in wrapped.iter().enumerate() {
        let (cxi, cyi) = grid.cell_of[i];
        let (x_cells, x_len) = neighbor_cells(cxi, grid.gx);
        let (y_cells, y_len) = neighbor_cells(cyi, grid.gy);

        let mut min_d2: Option<f64> = None;
        let mut z: usize = 0;
        let mut overlaps: u64 = 0;
        let mut acc_re = 0.0;
        let mut acc_im = 0.0;

        for &cy in &y_cells[..y_len] {
            let row = cy * grid.gx;
            for &cx in &x_cells[..x_len] {
                for &j in grid.bucket(row + cx) {
                    if j == i {
                        continue;
                    }
                    let [xj, yj] = wrapped[j];
                    let ddx = min_image(xj - xi, width);
                    let ddy = min_image(yj - yi, height);
                    let d2 = ddx * ddx + ddy * ddy;
                    min_d2 = Some(min_d2.map_or(d2, |best| best.min(d2)));
                    if d2 < r_overlap_sq {
                        overlaps += 1;
                    }
                    if d2 <= r_coord_sq {
                        z += 1;
                        if d2 > 0.0 {
                            let (u6_re, u6_im) = unit_hex_moment(ddx, ddy, d2);
                            acc_re += u6_re;
                            acc_im += u6_im;
                        }
                    }
                }
            }
        }

        let exact_d2 = if !fully_covered && min_d2.is_none_or(|d2| d2 > cell_min_sq) {
            grid.exact_nearest_d2(i, (cxi, cyi), &wrapped, [width, height], min_d2)
        } else {
            min_d2
        };

        let d1 = exact_d2.unwrap_or(0.0).sqrt();
        d1_over_d.push(d1 / diameter);
        overlap_sum += overlaps;
        coord_counts[z.min(8)] += 1;

        let denom = z.max(PSI6_MIN_DENOMINATOR) as f64;
        let psi_re = acc_re / denom;
        let psi_im = acc_im / denom;
        sum_abs_psi6 += (psi_re * psi_re + psi_im * psi_im).sqrt();
        global_psi6_re += psi_re;
        global_psi6_im += psi_im;
    }

    let n_f = n as f64;
    let mut coord_hist_0_to_8 = [0.0; 9];
    for (dst, &count) in coord_hist_0_to_8.iter_mut().zip(coord_counts.iter()) {
        *dst = count as f64 / n_f;
    }

    Ok(GeometryDiagnostics {
        count: n,
        diameter,
        median_d1_over_d: Some(median_in_place(&mut d1_over_d)),
        mean_overlaps_within_1d: overlap_sum as f64 / n_f,
        coord_hist_0_to_8,
        chain_fraction_z1_z2: coord_hist_0_to_8[1] + coord_hist_0_to_8[2],
        mean_abs_psi6: sum_abs_psi6 / n_f,
        global_abs_mean_psi6: (global_psi6_re * global_psi6_re + global_psi6_im * global_psi6_im)
            .sqrt()
            / n_f,
    })
}

/// Compute periodic geometry diagnostics from a committed [`World`] using its
/// configured `[contact] diameter`.
///
/// Returns `Ok(None)` when the world has no `[contact]` section instead of
/// inventing an arbitrary diameter.
///
/// # Errors
/// As [`geometry`].
pub fn geometry_of_world(world: &World) -> Result<Option<GeometryDiagnostics>, CoreError> {
    let Some(contact) = world.config().config().contact.as_ref() else {
        return Ok(None);
    };
    geometry_of_world_with_diameter(world, contact.diameter).map(Some)
}

/// Compute periodic geometry diagnostics from a committed [`World`] using an
/// explicit reference body diameter `diameter`.
///
/// # Errors
/// As [`geometry`].
pub fn geometry_of_world_with_diameter(
    world: &World,
    diameter: f64,
) -> Result<GeometryDiagnostics, CoreError> {
    let grid = world.field().grid();
    let positions: Vec<[f64; 2]> = world
        .population()
        .iter()
        .map(|organism| organism.position)
        .collect();
    geometry(
        &positions,
        [grid.world_width(), grid.world_height()],
        diameter,
    )
}

/// Compute periodic geometry diagnostics from a checkpoint [`Inspection`]
/// using body diameter `diameter`.
///
/// # Errors
/// As [`geometry`].
pub fn geometry_of_inspection(
    inspection: &Inspection,
    diameter: f64,
) -> Result<GeometryDiagnostics, CoreError> {
    let positions: Vec<[f64; 2]> = inspection
        .organisms
        .iter()
        .map(|organism| organism.position)
        .collect();
    geometry(
        &positions,
        [
            inspection.inspected.world.width,
            inspection.inspected.world.height,
        ],
        diameter,
    )
}

/// Compute `so2-motor` agency diagnostics directly from decoded [`MotorSample`]s.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] if any sample contains a nonfinite `m_e`,
/// `energy`, or `energy_change_rate`.
pub fn motor_agency(
    so2_motor_enabled: bool,
    total_count: usize,
    samples: &[MotorSample],
) -> Result<MotorAgencyDiagnostics, CoreError> {
    if !so2_motor_enabled {
        return Ok(MotorAgencyDiagnostics::unavailable(total_count));
    }
    let evaluated_count = samples.len();
    if evaluated_count == 0 {
        return Ok(MotorAgencyDiagnostics {
            so2_motor_enabled: true,
            total_count,
            evaluated_count: 0,
            mean_me: None,
            std_me: None,
            corr_me_dedt: None,
            corr_me_energy: None,
        });
    }

    let mut me_all = Vec::with_capacity(evaluated_count);
    let mut energy_all = Vec::with_capacity(evaluated_count);
    let mut me_for_dedt = Vec::with_capacity(evaluated_count);
    let mut dedt_vals = Vec::with_capacity(evaluated_count);
    let mut sum_me = 0.0;

    for (index, sample) in samples.iter().enumerate() {
        if !sample.m_e.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "motor sample m_e",
                detail: format!("sample {index} has nonfinite m_e {}", sample.m_e),
            });
        }
        if !sample.energy.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "motor sample energy",
                detail: format!("sample {index} has nonfinite energy {}", sample.energy),
            });
        }
        me_all.push(sample.m_e);
        energy_all.push(sample.energy);
        sum_me += sample.m_e;
        if let Some(dedt) = sample.energy_change_rate {
            if !dedt.is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "motor sample energy_change_rate",
                    detail: format!("sample {index} has nonfinite dE/dt {dedt}"),
                });
            }
            me_for_dedt.push(sample.m_e);
            dedt_vals.push(dedt);
        }
    }

    let mean_me = sum_me / evaluated_count as f64;
    if !mean_me.is_finite() {
        return Err(CoreError::NumericalInvalid {
            quantity: "motor sample m_e",
            detail: format!("mean m_e is not finite ({mean_me})"),
        });
    }
    let std_me = if evaluated_count >= 2 {
        let var = me_all
            .iter()
            .map(|&v| {
                let d = v - mean_me;
                d * d
            })
            .sum::<f64>()
            / evaluated_count as f64;
        if !var.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "motor sample m_e",
                detail: format!("variance of m_e is not finite ({var})"),
            });
        }
        Some(var.max(0.0).sqrt())
    } else {
        None
    };

    Ok(MotorAgencyDiagnostics {
        so2_motor_enabled: true,
        total_count,
        evaluated_count,
        mean_me: Some(mean_me),
        std_me,
        corr_me_dedt: pearson_corr(&me_for_dedt, &dedt_vals),
        corr_me_energy: pearson_corr(&me_all, &energy_all),
    })
}

/// Compute per-organism `so2-motor` agency diagnostics (`corr(m_e, dE/dt)` and
/// `corr(m_e, E)`) directly from a committed [`World`].
///
/// Respects `chemistry-only` controls (where effective `m_e = 0.0`, producing
/// `std_me = Some(0.0)` and `None` correlations).
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if a live organism's held output buffer is
/// shorter than the active `so2-motor` action slot; [`CoreError::NumericalInvalid`]
/// if any decoded value or energy field is nonfinite.
pub fn motor_agency_of_world(world: &World) -> Result<MotorAgencyDiagnostics, CoreError> {
    let population = world.population();
    let total_count = population.len();
    let Some(slot) = world.v2_rules().actions().so2_motor() else {
        return Ok(MotorAgencyDiagnostics::unavailable(total_count));
    };
    let decides_from_zero = ModelVariant::of(&world.config().config().model)?.decides_from_zero();
    let mut samples = Vec::with_capacity(total_count);
    for organism in population.iter() {
        let Some(outputs) = organism.held.outputs() else {
            continue;
        };
        let &raw_f32 = outputs.get(slot).ok_or_else(|| CoreError::LayoutMismatch {
            buffer: "held controller output",
            detail: format!(
                "{} holds {} outputs, shorter than so2-motor slot {slot}",
                organism.id,
                outputs.len()
            ),
        })?;
        let raw_me = if decides_from_zero {
            0.0
        } else {
            f64::from(raw_f32)
        };
        let m_e = numerics::tanh(raw_me).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "so2-motor output",
            detail: format!("{} motor output 0 is {raw_me}", organism.id),
        })?;
        samples.push(MotorSample {
            m_e,
            energy: organism.energy,
            energy_change_rate: Some(organism.energy_change_rate),
        });
    }
    motor_agency(true, total_count, &samples)
}

/// Compute per-organism `so2-motor` agency diagnostics (`corr(m_e, dE/dt)` and
/// `corr(m_e, E)`) from live [`OrganismSnapshot`]s.
///
/// Uses each snapshot's already-decoded `decoded.motor.energy_gradient` (which
/// already applies `chemistry-only` zeroing when active) and `organism.energy`
/// / `organism.energy_change_rate`. Because snapshots do not carry the world
/// config independently, `so2_motor_enabled` is reported `true` when at least
/// one snapshot carries a decoded `so2-motor` steering block.
///
/// # Errors
/// As [`motor_agency`].
pub fn motor_agency_of_snapshots(
    snapshots: &[OrganismSnapshot],
) -> Result<MotorAgencyDiagnostics, CoreError> {
    let total_count = snapshots.len();
    let mut samples = Vec::with_capacity(total_count);
    for snapshot in snapshots {
        let Some(motor) = snapshot.decoded.as_ref().and_then(|decoded| decoded.motor) else {
            continue;
        };
        samples.push(MotorSample {
            m_e: motor.energy_gradient,
            energy: snapshot.organism.energy,
            energy_change_rate: Some(snapshot.organism.energy_change_rate),
        });
    }
    let so2_motor_enabled = !samples.is_empty();
    motor_agency(so2_motor_enabled, total_count, &samples)
}

/// Compute per-organism `so2-motor` agency diagnostics from a checkpoint
/// [`Inspection`].
///
/// Computes `corr(m_e, E)`, `mean_me`, and `std_me` bit-identically to
/// [`motor_agency_of_world`], respecting `chemistry-only` controls. Because
/// checkpoint inspection records do not carry per-tick `energy_change_rate`,
/// `corr_me_dedt` is explicitly `None`.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if a held output vector is shorter than
/// [`SO2_MOTOR_ACTION_OFFSET`] + 1; [`CoreError::NumericalInvalid`] if any
/// raw output or energy is nonfinite.
pub fn motor_agency_of_inspection(
    inspection: &Inspection,
) -> Result<MotorAgencyDiagnostics, CoreError> {
    let total_count = inspection.organisms.len();
    let so2_motor_enabled = inspection
        .inspected
        .v2_features
        .as_deref()
        .is_some_and(|features| features.contains(&V2Feature::So2Motor.slug()));
    if !so2_motor_enabled {
        return Ok(MotorAgencyDiagnostics::unavailable(total_count));
    }
    let decides_from_zero = ModelVariant::of(&inspection.inspected.model)?.decides_from_zero();
    let mut samples = Vec::with_capacity(total_count);
    for organism in &inspection.organisms {
        let Some(held) = organism.held.as_ref() else {
            continue;
        };
        let &raw =
            held.outputs
                .get(SO2_MOTOR_ACTION_OFFSET)
                .ok_or_else(|| CoreError::LayoutMismatch {
                    buffer: "inspected held output",
                    detail: format!(
                        "organism {} holds {} outputs, shorter than so2-motor slot {}",
                        organism.id,
                        held.outputs.len(),
                        SO2_MOTOR_ACTION_OFFSET
                    ),
                })?;
        let raw_me = if decides_from_zero { 0.0 } else { raw };
        let m_e = numerics::tanh(raw_me).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "so2-motor output",
            detail: format!("organism {} motor output 0 is {raw_me}", organism.id),
        })?;
        samples.push(MotorSample {
            m_e,
            energy: organism.energy,
            energy_change_rate: None,
        });
    }
    motor_agency(true, total_count, &samples)
}

/// Compute both [`GeometryDiagnostics`] (when `[contact]` is configured) and
/// [`MotorAgencyDiagnostics`] for a committed [`World`].
///
/// # Errors
/// As [`geometry_of_world`] and [`motor_agency_of_world`].
pub fn of_world(world: &World) -> Result<V2Diagnostics, CoreError> {
    Ok(V2Diagnostics {
        geometry: geometry_of_world(world)?,
        motor_agency: motor_agency_of_world(world)?,
    })
}

/// Compute both [`GeometryDiagnostics`] (when `diameter` is provided) and
/// [`MotorAgencyDiagnostics`] for a checkpoint [`Inspection`].
///
/// # Errors
/// As [`geometry_of_inspection`] and [`motor_agency_of_inspection`].
pub fn of_inspection(
    inspection: &Inspection,
    diameter: Option<f64>,
) -> Result<V2Diagnostics, CoreError> {
    let geometry = match diameter {
        Some(d) => Some(geometry_of_inspection(inspection, d)?),
        None => None,
    };
    Ok(V2Diagnostics {
        geometry,
        motor_agency: motor_agency_of_inspection(inspection)?,
    })
}

/// Wrap `value` into `[0.0, span)`, clamping any IEEE-754 boundary rounding
/// from negative tiny values back to `0.0`.
#[inline]
fn wrap_coord(value: f64, span: f64) -> f64 {
    let wrapped = value.rem_euclid(span);
    if wrapped >= span { 0.0 } else { wrapped }
}

/// Periodic minimum-image displacement for wrapped coordinates in `[0, span)`.
#[inline]
fn min_image(delta: f64, span: f64) -> f64 {
    let half = 0.5 * span;
    if delta > half {
        delta - span
    } else if delta < -half {
        delta + span
    } else {
        delta
    }
}

/// Host-independent `(cos(6 θ), sin(6 θ))` via complex powers `(u_x + i u_y)^6`.
#[inline]
fn unit_hex_moment(ddx: f64, ddy: f64, d2: f64) -> (f64, f64) {
    let d = d2.sqrt();
    let ux = ddx / d;
    let uy = ddy / d;
    let u2_re = ux * ux - uy * uy;
    let u2_im = 2.0 * ux * uy;
    let u3_re = u2_re * ux - u2_im * uy;
    let u3_im = u2_re * uy + u2_im * ux;
    let u6_re = u3_re * u3_re - u3_im * u3_im;
    let u6_im = 2.0 * u3_re * u3_im;
    (u6_re, u6_im)
}

/// Exact statistical median of a non-empty slice of finite values.
fn median_in_place(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    if values.len().is_multiple_of(2) {
        0.5 * (values[mid - 1] + values[mid])
    } else {
        values[mid]
    }
}

/// Distinct periodic neighbor cell indices along one axis of length `count`.
#[inline]
fn neighbor_cells(center: usize, count: usize) -> ([usize; 3], usize) {
    match count {
        0 | 1 => ([0, 0, 0], 1),
        2 => ([center, 1 - center, 0], 2),
        _ => {
            let prev = if center == 0 { count - 1 } else { center - 1 };
            let next = if center + 1 == count { 0 } else { center + 1 };
            ([prev, center, next], 3)
        }
    }
}

/// Choose periodic spatial-grid resolution along one axis so `span / g >= r_coord`
/// whenever `span >= r_coord`, while bounding scratch size.
fn grid_cells(span: f64, r_coord: f64) -> usize {
    let raw = (span / r_coord).floor();
    if !raw.is_finite() || raw < 1.0 {
        1
    } else if raw >= MAX_GRID_SIDE as f64 {
        MAX_GRID_SIDE
    } else {
        raw as usize
    }
}

/// Cell index of a wrapped coordinate `coord ∈ [0, span)`.
#[inline]
fn cell_index(coord: f64, cell_size: f64, count: usize) -> usize {
    let raw = (coord / cell_size).floor();
    if !raw.is_finite() || raw <= 0.0 {
        0
    } else {
        (raw as usize).min(count - 1)
    }
}

/// Periodic cell index `(center + offset) mod count` with `|offset| < count`.
#[inline]
fn wrap_cell(center: usize, offset: isize, count: usize) -> usize {
    let mag = offset.unsigned_abs() % count;
    if offset >= 0 {
        (center + mag) % count
    } else {
        (center + count - mag) % count
    }
}

/// Flat CSR periodic spatial hash grid for O(N) neighbor lookup and exact
/// expanding-ring nearest-neighbor completion.
struct PeriodicGrid {
    gx: usize,
    gy: usize,
    cell_w: f64,
    cell_h: f64,
    cell_of: Vec<(usize, usize)>,
    starts: Vec<usize>,
    order: Vec<usize>,
}

impl PeriodicGrid {
    fn build(wrapped: &[[f64; 2]], extent: [f64; 2], r_coord: f64) -> Self {
        let [width, height] = extent;
        let gx = grid_cells(width, r_coord);
        let gy = grid_cells(height, r_coord);
        let cell_w = width / gx as f64;
        let cell_h = height / gy as f64;
        let cell_count = gx * gy;

        let mut cell_of = Vec::with_capacity(wrapped.len());
        let mut counts = vec![0_usize; cell_count];
        for &[x, y] in wrapped {
            let cx = cell_index(x, cell_w, gx);
            let cy = cell_index(y, cell_h, gy);
            cell_of.push((cx, cy));
            counts[cy * gx + cx] += 1;
        }

        let mut starts = Vec::with_capacity(cell_count + 1);
        let mut offset = 0_usize;
        starts.push(0);
        for &count in &counts {
            offset += count;
            starts.push(offset);
        }

        let mut cursor = starts[..cell_count].to_vec();
        let mut order = vec![0_usize; wrapped.len()];
        for (i, &(cx, cy)) in cell_of.iter().enumerate() {
            let bucket = cy * gx + cx;
            order[cursor[bucket]] = i;
            cursor[bucket] += 1;
        }

        Self {
            gx,
            gy,
            cell_w,
            cell_h,
            cell_of,
            starts,
            order,
        }
    }

    #[inline]
    fn bucket(&self, cell: usize) -> &[usize] {
        &self.order[self.starts[cell]..self.starts[cell + 1]]
    }

    /// Complete the exact global nearest-neighbor squared distance when the
    /// initial 3×3 neighborhood did not certify a neighbor within `min(cell_w, cell_h)`.
    fn exact_nearest_d2(
        &self,
        index: usize,
        center: (usize, usize),
        wrapped: &[[f64; 2]],
        extent: [f64; 2],
        initial_min_d2: Option<f64>,
    ) -> Option<f64> {
        let [width, height] = extent;
        let [xi, yi] = wrapped[index];
        let (cxi, cyi) = center;
        let cell_min = self.cell_w.min(self.cell_h);

        let mut best = initial_min_d2;
        let mut ring: usize = 2;

        while 2 * ring < self.gx && 2 * ring < self.gy {
            let r = ring as isize;
            for oy in -r..=r {
                let cy = wrap_cell(cyi, oy, self.gy);
                let row = cy * self.gx;
                if oy.unsigned_abs() == ring {
                    for ox in -r..=r {
                        let cx = wrap_cell(cxi, ox, self.gx);
                        for &j in self.bucket(row + cx) {
                            if j == index {
                                continue;
                            }
                            let [xj, yj] = wrapped[j];
                            let ddx = min_image(xj - xi, width);
                            let ddy = min_image(yj - yi, height);
                            let d2 = ddx * ddx + ddy * ddy;
                            best = Some(best.map_or(d2, |cur| cur.min(d2)));
                        }
                    }
                } else {
                    for ox in [-r, r] {
                        let cx = wrap_cell(cxi, ox, self.gx);
                        for &j in self.bucket(row + cx) {
                            if j == index {
                                continue;
                            }
                            let [xj, yj] = wrapped[j];
                            let ddx = min_image(xj - xi, width);
                            let ddy = min_image(yj - yi, height);
                            let d2 = ddx * ddx + ddy * ddy;
                            best = Some(best.map_or(d2, |cur| cur.min(d2)));
                        }
                    }
                }
            }

            let bound = ring as f64 * cell_min;
            if best.is_some_and(|d2| d2 <= bound * bound) {
                return best;
            }
            ring += 1;
        }

        // Expanding ring reached half the torus along an axis: exact scan over all organisms.
        for (j, &[xj, yj]) in wrapped.iter().enumerate() {
            if j == index {
                continue;
            }
            let ddx = min_image(xj - xi, width);
            let ddy = min_image(yj - yi, height);
            let d2 = ddx * ddx + ddy * ddy;
            best = Some(best.map_or(d2, |cur| cur.min(d2)));
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::*;

    /// Independent O(N²) brute-force periodic geometry oracle (no spatial grid,
    /// standard `atan2`/`cos`/`sin`) to cross-check the production spatial hash
    /// and algebraic hexatic power implementation.
    fn brute_force_geometry_oracle(
        positions: &[[f64; 2]],
        extent: [f64; 2],
        diameter: f64,
    ) -> GeometryDiagnostics {
        let [width, height] = extent;
        let n = positions.len();
        let r_coord = COORDINATION_RADIUS_MULTIPLIER * diameter;
        let r_coord_sq = r_coord * r_coord;
        let r_overlap_sq = diameter * diameter;
        let mut d1_vals = Vec::with_capacity(n);
        let mut overlaps_total = 0_usize;
        let mut hist = [0_usize; 9];
        let mut sum_abs_psi6 = 0.0;
        let mut sum_re = 0.0;
        let mut sum_im = 0.0;

        for (i, &[xi, yi]) in positions.iter().enumerate() {
            let mut min_d2 = f64::INFINITY;
            let mut z = 0_usize;
            let mut acc_re = 0.0;
            let mut acc_im = 0.0;
            for (j, &[xj, yj]) in positions.iter().enumerate() {
                if i == j {
                    continue;
                }
                let mut dx = (xj - xi).rem_euclid(width);
                if dx > 0.5 * width {
                    dx -= width;
                }
                let mut dy = (yj - yi).rem_euclid(height);
                if dy > 0.5 * height {
                    dy -= height;
                }
                let d2 = dx * dx + dy * dy;
                if d2 < min_d2 {
                    min_d2 = d2;
                }
                if d2 < r_overlap_sq {
                    overlaps_total += 1;
                }
                if d2 <= r_coord_sq {
                    z += 1;
                    if d2 > 0.0 {
                        let theta = dy.atan2(dx);
                        acc_re += (6.0 * theta).cos();
                        acc_im += (6.0 * theta).sin();
                    }
                }
            }
            d1_vals.push(min_d2.sqrt() / diameter);
            hist[z.min(8)] += 1;
            let denom = z.max(PSI6_MIN_DENOMINATOR) as f64;
            let re = acc_re / denom;
            let im = acc_im / denom;
            sum_abs_psi6 += re.hypot(im);
            sum_re += re;
            sum_im += im;
        }
        d1_vals.sort_by(f64::total_cmp);
        let mid = n / 2;
        let median = if n.is_multiple_of(2) {
            0.5 * (d1_vals[mid - 1] + d1_vals[mid])
        } else {
            d1_vals[mid]
        };
        let mut norm_hist = [0.0; 9];
        for (dst, &count) in norm_hist.iter_mut().zip(hist.iter()) {
            *dst = count as f64 / n as f64;
        }
        GeometryDiagnostics {
            count: n,
            diameter,
            median_d1_over_d: Some(median),
            mean_overlaps_within_1d: overlaps_total as f64 / n as f64,
            coord_hist_0_to_8: norm_hist,
            chain_fraction_z1_z2: norm_hist[1] + norm_hist[2],
            mean_abs_psi6: sum_abs_psi6 / n as f64,
            global_abs_mean_psi6: sum_re.hypot(sum_im) / n as f64,
        }
    }

    #[test]
    fn analytical_periodic_hexagonal_crystal_oracle() {
        // Periodic triangular lattice of spacing a = 1.05 * D: every site has
        // exactly z = 6 neighbors within 1.3 D, zero overlaps (< D), and exact
        // |psi_6(i)| = 1, |mean psi_6| = 1.
        let diameter = 1.5;
        let a = 1.05 * diameter;
        let dy = a * 0.5 * 3.0_f64.sqrt();
        let nx = 8_usize;
        let ny = 8_usize; // even so periodic vertical seam tiles cleanly
        let extent = [nx as f64 * a, ny as f64 * dy];
        let mut positions = Vec::with_capacity(nx * ny);
        for row in 0..ny {
            let x_shift = if row.is_multiple_of(2) { 0.0 } else { 0.5 * a };
            for col in 0..nx {
                positions.push([col as f64 * a + x_shift, row as f64 * dy]);
            }
        }

        let got = geometry(&positions, extent, diameter).expect("hex crystal");
        assert_eq!(got.count, nx * ny);
        assert!((got.median_d1_over_d.expect("median") - 1.05).abs() < 1e-12);
        assert_eq!(got.mean_overlaps_within_1d, 0.0);
        assert_eq!(got.coord_hist_0_to_8[6], 1.0);
        assert_eq!(got.chain_fraction_z1_z2, 0.0);
        assert!((got.mean_abs_psi6 - 1.0).abs() < 1e-12);
        assert!((got.global_abs_mean_psi6 - 1.0).abs() < 1e-12);

        // JSON field names match V2-10 / V2-17 specification.
        let json = serde_json::to_value(&got).expect("serialize");
        assert!(json.get("median_d1_over_D").is_some());
        assert!(json.get("mean_overlaps_within_1D").is_some());
        assert!(json.get("coord_hist_0_to_8").is_some());
        assert!(json.get("chain_fraction_z1_z2").is_some());
        assert!(json.get("mean_abs_psi6").is_some());
        assert!(json.get("global_abs_mean_psi6").is_some());
    }

    #[test]
    fn analytical_24_cell_hollow_ring_oracle() {
        // 24-cell 1-cell-wide circular ring with chord spacing = D:
        // every cell has z = 2 neighbors (at ±165° relative interior angle),
        // 6 * 165° = 990° ≡ 270° => |1 + e^{i 270°}| / max(2, 6) = sqrt(2) / 6 ≈ 0.23570226,
        // global |mean psi_6| = 0 by C_24 symmetry, chain_fraction = 1.0.
        let n = 24_usize;
        let diameter = 1.0;
        let chord = 1.01 * diameter;
        let radius = chord / (2.0 * (PI / n as f64).sin());
        let center = [25.0, 25.0];
        let positions: Vec<[f64; 2]> = (0..n)
            .map(|k| {
                let angle = 2.0 * PI * (k as f64) / (n as f64);
                [
                    center[0] + radius * angle.cos(),
                    center[1] + radius * angle.sin(),
                ]
            })
            .collect();

        let got = geometry(&positions, [50.0, 50.0], diameter).expect("24-cell ring");
        let expected_psi6 = 2.0_f64.sqrt() / 6.0;
        assert!((got.median_d1_over_d.expect("median") - 1.01).abs() < 1e-12);
        assert_eq!(got.mean_overlaps_within_1d, 0.0);
        assert_eq!(got.coord_hist_0_to_8[2], 1.0);
        assert_eq!(got.chain_fraction_z1_z2, 1.0);
        assert!((got.mean_abs_psi6 - expected_psi6).abs() < 1e-12);
        assert!(got.global_abs_mean_psi6 < 1e-12);
    }

    #[test]
    fn jammed_overlapping_pairs_and_periodic_seam_wrap() {
        // Two overlapping pairs: one interior, one straddling the x=0 / x=20
        // periodic seam at separation 0.5 D.
        let diameter = 2.0;
        let positions = [
            [0.4, 5.0],
            [19.4, 5.0], // periodic dx = -1.0 = 0.5 D
            [10.0, 10.0],
            [10.0, 11.0], // dy = 1.0 = 0.5 D
        ];
        let got = geometry(&positions, [20.0, 20.0], diameter).expect("pairs");
        assert!((got.median_d1_over_d.expect("median") - 0.5).abs() < 1e-12);
        assert!((got.mean_overlaps_within_1d - 1.0).abs() < 1e-12);
        assert_eq!(got.coord_hist_0_to_8[1], 1.0);
        assert_eq!(got.chain_fraction_z1_z2, 1.0);
    }

    #[test]
    fn spatial_hash_matches_brute_force_oracle_across_sparse_anisotropic_and_coincident() {
        // Deterministic mixed cloud: coincident pair, tight cluster, sparse
        // outliers separated > 4 * r_coord (exercising expanding rings and
        // full-torus fallback), and anisotropic small-grid axes (gx = 2, gy = 9).
        let diameter = 1.0;
        let cases: &[([f64; 2], &[[f64; 2]])] = &[
            (
                [30.0, 30.0],
                &[
                    [0.1, 0.1],
                    [29.9, 29.9],
                    [5.0, 5.0],
                    [5.0, 5.0], // coincident pair: d2 == 0
                    [5.6, 5.2],
                    [15.0, 15.0], // isolated > 10 D away
                    [22.5, 4.0],  // isolated
                ],
            ),
            (
                [3.0, 14.0], // gx = 2, gy = 10 (tests gx < 3 non-double-counting)
                &[[0.2, 0.5], [2.7, 0.6], [1.5, 7.0], [1.5, 13.5]],
            ),
            (
                [1.0, 1.0], // gx = 1, gy = 1 (extent < r_coord)
                &[[0.1, 0.2], [0.8, 0.9], [0.4, 0.5]],
            ),
        ];

        for &(extent, pts) in cases {
            let got = geometry(pts, extent, diameter).expect("spatial hash");
            let oracle = brute_force_geometry_oracle(pts, extent, diameter);
            assert_eq!(got.count, oracle.count);
            assert!(
                (got.median_d1_over_d.expect("got") - oracle.median_d1_over_d.expect("oracle"))
                    .abs()
                    < 1e-12
            );
            assert!((got.mean_overlaps_within_1d - oracle.mean_overlaps_within_1d).abs() < 1e-12);
            assert_eq!(got.coord_hist_0_to_8, oracle.coord_hist_0_to_8);
            assert!((got.chain_fraction_z1_z2 - oracle.chain_fraction_z1_z2).abs() < 1e-12);
            assert!((got.mean_abs_psi6 - oracle.mean_abs_psi6).abs() < 1e-12);
            assert!((got.global_abs_mean_psi6 - oracle.global_abs_mean_psi6).abs() < 1e-12);
        }
    }

    #[test]
    fn geometry_edge_cases_and_invalid_inputs() {
        let empty = geometry(&[], [10.0, 10.0], 1.0).expect("empty");
        assert_eq!(empty.count, 0);
        assert_eq!(empty.median_d1_over_d, None);
        assert_eq!(empty.mean_overlaps_within_1d, 0.0);
        assert_eq!(empty.coord_hist_0_to_8, [0.0; 9]);

        let single = geometry(&[[3.0, 4.0]], [10.0, 10.0], 1.0).expect("single");
        assert_eq!(single.count, 1);
        assert_eq!(single.median_d1_over_d, None);
        assert_eq!(single.coord_hist_0_to_8[0], 1.0);
        assert_eq!(single.chain_fraction_z1_z2, 0.0);

        assert!(geometry(&[], [0.0, 10.0], 1.0).is_err());
        assert!(geometry(&[], [10.0, f64::NAN], 1.0).is_err());
        assert!(geometry(&[], [10.0, 10.0], 0.0).is_err());
        assert!(geometry(&[], [10.0, 10.0], -1.0).is_err());
        assert!(geometry(&[], [10.0, 10.0], 1e200).is_err());
        assert!(geometry(&[], [10.0, 10.0], 1e-200).is_err());
        assert!(geometry(&[], [1e200, 1e200], 1.0).is_err());
        assert!(geometry(&[[f64::INFINITY, 1.0]], [10.0, 10.0], 1.0).is_err());
    }

    #[test]
    fn pearson_corr_known_answers_and_degenerate_cases() {
        assert!(
            (pearson_corr(&[1.0, 2.0, 3.0, 4.0], &[2.0, 4.0, 6.0, 8.0]).expect("+1") - 1.0).abs()
                < 1e-12
        );
        assert!(
            (pearson_corr(&[1.0, 2.0, 3.0, 4.0], &[8.0, 6.0, 4.0, 2.0]).expect("-1") + 1.0).abs()
                < 1e-12
        );
        // Symmetric orthogonal pair -> exact 0 correlation.
        assert!(
            pearson_corr(&[-1.0, 0.0, 1.0], &[1.0, 0.0, 1.0])
                .expect("0")
                .abs()
                < 1e-12
        );
        // Degenerate cases must return None.
        assert_eq!(pearson_corr(&[], &[]), None);
        assert_eq!(pearson_corr(&[1.0], &[2.0]), None);
        assert_eq!(pearson_corr(&[1.0, 2.0], &[1.0]), None);
        assert_eq!(pearson_corr(&[0.0, 0.0, 0.0], &[1.0, 2.0, 3.0]), None);
        assert_eq!(pearson_corr(&[1.0, f64::NAN], &[1.0, 2.0]), None);
    }

    #[test]
    fn motor_agency_direct_samples_and_missing_dedt() {
        let samples = [
            MotorSample {
                m_e: -0.5,
                energy: 1.0,
                energy_change_rate: Some(0.5),
            },
            MotorSample {
                m_e: 0.0,
                energy: 2.0,
                energy_change_rate: Some(0.0),
            },
            MotorSample {
                m_e: 0.5,
                energy: 3.0,
                energy_change_rate: Some(-0.5),
            },
        ];
        let got = motor_agency(true, 4, &samples).expect("motor agency");
        assert!(got.so2_motor_enabled);
        assert_eq!(got.total_count, 4);
        assert_eq!(got.evaluated_count, 3);
        assert!(got.mean_me.expect("mean").abs() < 1e-12);
        assert!((got.std_me.expect("std") - (0.5_f64 / 3.0).sqrt()).abs() < 1e-12);
        assert!((got.corr_me_dedt.expect("corr dedt") + 1.0).abs() < 1e-12);
        assert!((got.corr_me_energy.expect("corr E") - 1.0).abs() < 1e-12);

        // When energy_change_rate is unavailable (checkpoint inspection),
        // corr_me_dedt is None while corr_me_energy stays intact.
        let no_dedt: Vec<MotorSample> = samples
            .iter()
            .map(|s| MotorSample {
                energy_change_rate: None,
                ..*s
            })
            .collect();
        let got_no_dedt = motor_agency(true, 3, &no_dedt).expect("no dedt");
        assert_eq!(got_no_dedt.corr_me_dedt, None);
        assert!((got_no_dedt.corr_me_energy.expect("corr E") - 1.0).abs() < 1e-12);

        // Disabled feature returns explicit unavailable state.
        let off = motor_agency(false, 3, &samples).expect("disabled");
        assert!(!off.so2_motor_enabled);
        assert_eq!(off.evaluated_count, 0);
        assert_eq!(off.corr_me_dedt, None);
        assert_eq!(off.corr_me_energy, None);

        // Overflowing mean or variance on extreme finite samples must be refused.
        let overflow_mean = [
            MotorSample {
                m_e: 1.0e308,
                energy: 1.0,
                energy_change_rate: None,
            },
            MotorSample {
                m_e: 1.0e308,
                energy: 2.0,
                energy_change_rate: None,
            },
        ];
        assert!(matches!(
            motor_agency(true, 2, &overflow_mean),
            Err(CoreError::NumericalInvalid { .. })
        ));
        let overflow_var = [
            MotorSample {
                m_e: 1.0e308,
                energy: 1.0,
                energy_change_rate: None,
            },
            MotorSample {
                m_e: -1.0e308,
                energy: 2.0,
                energy_change_rate: None,
            },
        ];
        assert!(matches!(
            motor_agency(true, 2, &overflow_var),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }
}
