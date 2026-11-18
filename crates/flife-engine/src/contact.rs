//! Soft-disk contact for the `field-ecology-v1-contact` models (plan §5.4's
//! "direct collisions … define later model variants").
//!
//! Under a contact model every organism is a disk of diameter `D`, the
//! configuration's `reproduction.child_position_offset_radius`: a newborn is
//! placed within one body of its parent. After phase 8, each organism that
//! survives the tick is displaced by its overlaps with every other survivor,
//! all computed from the settled positions before any displacement:
//!
//! ```text
//! Δ_i = Σ_{j ≠ i, d_ij < D} (k/2) (D − d_ij) u_ij,     k = CONTACT_STIFFNESS,
//! ```
//!
//! where `d_ij` is the minimum-image distance and `u_ij` the unit vector from
//! `j` to `i`. Two organisms at one point separate along `±x`, the higher
//! identity towards `+x`. `Δ_i` is then clamped radially to
//! `min(D/2, ½ min(h_x, h_y))`, and the position wrapped.
//!
//! Contact moves positions only. It changes no velocity and no energy, so the
//! ledger is the same with or without it. Emissions and births of the tick
//! use the settled positions, before contact.
//!
//! Each sum runs over the neighbour bins in a fixed order, `(dy, dx)` from
//! `(−1, −1)` to `(1, 1)`, and within a bin in identity order, so a world's
//! contact displacement is the same at any worker count.

use flife_core::{CoreError, ValidatedConfig};
use flife_field::FieldGrid;

use crate::world::ModelVariant;

/// The fraction `k` of each pair's overlap resolved per tick, split equally
/// between the two organisms. A model constant of the contact variants.
pub const CONTACT_STIFFNESS: f64 = 0.5;

/// The most bins a side may have; a smaller diameter uses coarser bins,
/// which find the same neighbours.
const MAXIMUM_BINS_PER_SIDE: usize = 2048;

/// Reusable buffers for [`relax`].
#[derive(Debug, Default)]
pub struct ContactScratch {
    /// Start of each bin's members in `members`, plus one end.
    starts: Vec<usize>,
    /// Indices into the positions, grouped by bin, in index order.
    members: Vec<usize>,
    /// Each position's bin.
    bins: Vec<usize>,
}

/// An organism's body radius in world units under `config`'s model: half
/// the contact diameter for a contact model, none for any other, whose
/// organisms are points.
///
/// # Errors
/// [`CoreError::UnknownAlgorithm`] for a model this build does not run.
pub fn body_radius(config: &ValidatedConfig) -> Result<Option<f64>, CoreError> {
    let model = config.config();
    Ok(ModelVariant::of(&model.model)?
        .contact()
        .then_some(0.5 * model.reproduction.child_position_offset_radius))
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
    /// Each one's displacement, zero for those that die.
    pub displacements: Vec<[f64; 2]>,
    scratch: ContactScratch,
}

impl ContactWork {
    /// Buffers for `rows` organisms on `grid` at `diameter`, reserved now so
    /// that no later tick allocates (FL-043).
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when a reservation fails.
    pub fn with_capacity(grid: &FieldGrid, diameter: f64, rows: usize) -> Result<Self, CoreError> {
        let bins =
            bins_along(grid.world_width(), diameter) * bins_along(grid.world_height(), diameter);
        let mut work = Self::default();
        let fail = |buffer: &'static str, count: usize, size: usize| CoreError::AllocationFailed {
            buffer,
            bytes: u64::try_from(count.saturating_mul(size)).unwrap_or(u64::MAX),
        };
        work.positions
            .try_reserve_exact(rows)
            .map_err(|_| fail("contact positions", rows, 16))?;
        work.ids
            .try_reserve_exact(rows)
            .map_err(|_| fail("contact identities", rows, 8))?;
        work.alive
            .try_reserve_exact(rows)
            .map_err(|_| fail("contact fates", rows, 1))?;
        work.displacements
            .try_reserve_exact(rows)
            .map_err(|_| fail("contact displacements", rows, 16))?;
        work.scratch
            .members
            .try_reserve_exact(rows)
            .map_err(|_| fail("contact members", rows, 8))?;
        work.scratch
            .bins
            .try_reserve_exact(rows)
            .map_err(|_| fail("contact bins", rows, 8))?;
        work.scratch
            .starts
            .try_reserve_exact(bins + 1)
            .map_err(|_| fail("contact bin starts", bins + 1, 8))?;
        Ok(work)
    }

    /// Bytes the buffers hold.
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        let bytes = |capacity: usize, size: usize| {
            u64::try_from(capacity.saturating_mul(size)).unwrap_or(u64::MAX)
        };
        [
            bytes(self.positions.capacity(), 16),
            bytes(self.ids.capacity(), 8),
            bytes(self.alive.capacity(), 1),
            bytes(self.displacements.capacity(), 16),
            bytes(self.scratch.members.capacity(), 8),
            bytes(self.scratch.bins.capacity(), 8),
            bytes(self.scratch.starts.capacity(), 8),
        ]
        .into_iter()
        .fold(0_u64, u64::saturating_add)
    }

    /// [`relax`] on the gathered survivors, into [`Self::displacements`].
    ///
    /// # Errors
    /// As [`relax`].
    pub fn relax(&mut self, grid: &FieldGrid, diameter: f64) -> Result<(), CoreError> {
        let Self {
            positions,
            ids,
            alive,
            displacements,
            scratch,
        } = self;
        relax(
            grid,
            diameter,
            positions,
            ids,
            alive,
            displacements,
            scratch,
        )
    }
}

/// The bins along one side: as many as fit cells at least `diameter` wide.
fn bins_along(length: f64, diameter: f64) -> usize {
    let fit = (length / diameter).floor();
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

/// Every survivor's contact displacement, written into `displacements`
/// (aligned with `positions`; zero where `alive` is false).
///
/// `positions` are wrapped, in identity order, and `ids` their identities in
/// the same order, used only to separate coincident organisms.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a nonpositive or nonfinite diameter,
/// or a position the grid does not hold.
pub fn relax(
    grid: &FieldGrid,
    diameter: f64,
    positions: &[[f64; 2]],
    ids: &[u64],
    alive: &[bool],
    displacements: &mut Vec<[f64; 2]>,
    scratch: &mut ContactScratch,
) -> Result<(), CoreError> {
    if !(diameter.is_finite() && diameter > 0.0) {
        return Err(CoreError::NumericalInvalid {
            quantity: "contact diameter",
            detail: format!(
                "reproduction.child_position_offset_radius {diameter:?} is not a positive \
                 body diameter, which a contact model needs"
            ),
        });
    }
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
    let (width, height) = (grid.world_width(), grid.world_height());
    let (bins_x, bins_y) = (bins_along(width, diameter), bins_along(height, diameter));
    let limit = (0.5 * diameter).min(0.5 * grid.cell_width().min(grid.cell_height()));

    // Counting sort of the survivors by bin, stable in identity order.
    let ContactScratch {
        starts,
        members,
        bins,
    } = scratch;
    starts.clear();
    starts.resize(bins_x * bins_y + 1, 0);
    bins.clear();
    for (&[x, y], &live) in positions.iter().zip(alive) {
        let bin = if live {
            if !(0.0..width).contains(&x) || !(0.0..height).contains(&y) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "contact position",
                    detail: format!("[{x:?}, {y:?}] is not wrapped"),
                });
            }
            bin_of(y, height, bins_y) * bins_x + bin_of(x, width, bins_x)
        } else {
            usize::MAX
        };
        bins.push(bin);
        if bin != usize::MAX {
            starts[bin + 1] += 1;
        }
    }
    for bin in 0..bins_x * bins_y {
        starts[bin + 1] += starts[bin];
    }
    members.clear();
    members.resize(starts[bins_x * bins_y], 0);
    // Fill each bin from its end, walking the organisms backwards, so that
    // `starts` ends as each bin's first member without a second buffer.
    for (index, &bin) in bins.iter().enumerate().rev() {
        if bin != usize::MAX {
            starts[bin + 1] -= 1;
            members[starts[bin + 1]] = index;
        }
    }
    // Now `starts[b + 1]` is bin b's start; shift down by one bin.
    starts.remove(0);
    starts.push(members.len());

    displacements.clear();
    displacements.resize(count, [0.0, 0.0]);
    let factor = 0.5 * CONTACT_STIFFNESS;
    for index in 0..count {
        let bin = bins[index];
        if bin == usize::MAX {
            continue;
        }
        let here = positions[index];
        let (bx, by) = (bin % bins_x, bin / bins_x);
        let (rows, row_count) = neighbours(by, bins_y);
        let (columns, column_count) = neighbours(bx, bins_x);
        let mut sum = [0.0_f64, 0.0_f64];
        for &row in &rows[..row_count] {
            for &column in &columns[..column_count] {
                let neighbour = row * bins_x + column;
                for &other in &members[starts[neighbour]..starts[neighbour + 1]] {
                    if other == index {
                        continue;
                    }
                    // From the other organism to this one.
                    let [dx, dy] = grid.displacement(positions[other], here)?;
                    let distance = dx.hypot(dy);
                    if distance >= diameter {
                        continue;
                    }
                    let push = factor * (diameter - distance);
                    let (ux, uy) = if distance > 0.0 {
                        (dx / distance, dy / distance)
                    } else if ids[index] > ids[other] {
                        (1.0, 0.0)
                    } else {
                        (-1.0, 0.0)
                    };
                    sum[0] += push * ux;
                    sum[1] += push * uy;
                }
            }
        }
        let length = sum[0].hypot(sum[1]);
        if length > limit {
            let scale = limit / length;
            sum = [sum[0] * scale, sum[1] * scale];
        }
        displacements[index] = sum;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(side: f64, cells: u32) -> FieldGrid {
        FieldGrid::new(cells, cells, side, side).expect("a valid grid")
    }

    fn run(grid: &FieldGrid, diameter: f64, positions: &[[f64; 2]]) -> Vec<[f64; 2]> {
        let ids: Vec<u64> = (0..positions.len() as u64).collect();
        let alive = vec![true; positions.len()];
        let mut out = Vec::new();
        relax(
            grid,
            diameter,
            positions,
            &ids,
            &alive,
            &mut out,
            &mut ContactScratch::default(),
        )
        .expect("contact");
        out
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
        let ids = [0, 1];
        let mut out = Vec::new();
        relax(
            &grid,
            0.5,
            &[[4.0, 4.0], [4.1, 4.0]],
            &ids,
            &[true, false],
            &mut out,
            &mut ContactScratch::default(),
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

    #[test]
    fn the_sum_matches_an_all_pairs_oracle() {
        let grid = grid(8.0, 8);
        let mut positions = Vec::new();
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..400 {
            let mut next = || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 11) as f64 / (1_u64 << 53) as f64
            };
            positions.push([next() * 8.0, next() * 8.0]);
        }
        let out = run(&grid, 0.45, &positions);
        for (index, &[x, y]) in positions.iter().enumerate() {
            let mut sum = [0.0, 0.0];
            for (other, &position) in positions.iter().enumerate() {
                if other == index {
                    continue;
                }
                let [dx, dy] = grid.displacement(position, [x, y]).expect("wrapped");
                let distance = dx.hypot(dy);
                if distance < 0.45 && distance > 0.0 {
                    let push = 0.25 * (0.45 - distance);
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
}
