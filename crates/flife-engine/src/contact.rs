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
//! Contact moves positions only. It changes no velocity and no energy, so the
//! ledger is the same with or without it. Emissions and births of the tick
//! use the settled positions, before contact.
//!
//! Each sum runs over the neighbour bins, at least the reach wide, in a fixed
//! order, `(dy, dx)` from `(−1, −1)` to `(1, 1)`, and within a bin in identity
//! order, so a world's contact displacement is the same at any worker count.

use flife_core::{CoreError, ValidatedConfig};
use flife_field::FieldGrid;
use flife_genome::AFFINITY_TERMS;

/// The most bins a side may have; a smaller reach uses coarser bins, which
/// find the same neighbours.
const MAXIMUM_BINS_PER_SIDE: usize = 2048;

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
        let rules = Self {
            diameter: contact.diameter,
            stiffness: contact.stiffness,
            adhesion: contact.adhesion,
            range: contact.adhesion_range,
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
            && self.range >= 1.0;
        if ok {
            Ok(())
        } else {
            Err(CoreError::NumericalInvalid {
                quantity: "contact",
                detail: format!("{self:?}: D > 0, k in (0, 1], k_a in [0, 1] and R >= 1"),
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
    /// Each one's unit affinity direction, when bodies adhere.
    pub affinities: Vec<[f32; AFFINITY_TERMS]>,
    /// Each one's displacement, zero for those that die.
    pub displacements: Vec<[f64; 2]>,
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
        let reach = rules.reach();
        let bins = bins_along(grid.world_width(), reach) * bins_along(grid.world_height(), reach);
        let mut work = Self::default();
        reserve(&mut work.positions, rows, "contact positions")?;
        reserve(&mut work.ids, rows, "contact identities")?;
        reserve(&mut work.alive, rows, "contact fates")?;
        if rules.adheres() {
            reserve(&mut work.affinities, rows, "contact affinities")?;
        }
        reserve(&mut work.displacements, rows, "contact displacements")?;
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
            bytes(&self.displacements),
            bytes(&self.scratch.members),
            bytes(&self.scratch.bins),
            bytes(&self.scratch.starts),
        ]
        .into_iter()
        .fold(0_u64, u64::saturating_add)
    }

    /// [`relax`] on the gathered survivors, into [`Self::displacements`].
    ///
    /// # Errors
    /// As [`relax`].
    pub fn relax(&mut self, grid: &FieldGrid, rules: &ContactRules) -> Result<(), CoreError> {
        let Self {
            positions,
            ids,
            alive,
            affinities,
            displacements,
            scratch,
        } = self;
        let affinities = rules.adheres().then_some(affinities.as_slice());
        relax(
            grid,
            rules,
            Bodies {
                positions,
                ids,
                alive,
                affinities,
            },
            displacements,
            scratch,
        )
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
fn cosine(a: &[f32; AFFINITY_TERMS], b: &[f32; AFFINITY_TERMS]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| f64::from(*x) * f64::from(*y))
        .sum()
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

/// Every survivor's contact displacement, written into `displacements`
/// (aligned with the bodies; zero where a body dies).
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
    } = *rules;
    let reach = rules.reach();
    let (width, height) = (grid.world_width(), grid.world_height());
    let (bins_x, bins_y) = (bins_along(width, reach), bins_along(height, reach));
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
    // Fill each bin from its end, walking the bodies backwards, so that
    // `starts[b + 1]` ends as bin b's first member without a second buffer.
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
    let push_factor = 0.5 * stiffness;
    let pull_factor = if range > 1.0 {
        0.5 * adhesion / ((range - 1.0) * diameter)
    } else {
        0.0
    };
    let reach_distance = diameter * range;
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
                    // From the other body to this one.
                    let [dx, dy] = grid.displacement(positions[other], here)?;
                    let distance = dx.hypot(dy);
                    let push = if distance < diameter {
                        push_factor * (diameter - distance)
                    } else if let Some(affinities) =
                        affinities.filter(|_| distance < reach_distance)
                    {
                        let alike = cosine(&affinities[index], &affinities[other]).max(0.0);
                        -pull_factor
                            * alike
                            * alike
                            * (distance - diameter)
                            * (reach_distance - distance)
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

    const PLAIN: ContactRules = ContactRules {
        diameter: 0.5,
        stiffness: 0.5,
        adhesion: 0.0,
        range: 1.0,
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
            )
            .is_err()
        );
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
}
