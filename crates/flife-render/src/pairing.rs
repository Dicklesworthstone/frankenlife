//! Matching organisms between two epochs by stable identity (plan §13.5,
//! C.15).
//!
//! Interpolation pairs an organism with itself, never with whatever reused
//! its storage slot. Both epochs list identities in increasing order, so
//! [`pairs`] is one merge. An identity is never reissued, so an
//! identity present only in the later epoch was born after the earlier
//! one, and one present only in the earlier epoch died.
//!
//! Latest-value publication can skip many ticks. A [`Gap`] states whether
//! minimum-image interpolation is still safe: it is only if no organism can
//! have moved half of either domain extent in the gap. Otherwise a viewer
//! must snap to the later state. [`interpolate`] draws an epoch between two
//! by these rules (FL-020).

use flife_core::CoreError;

use crate::epoch::Epoch;

/// A relative margin on the travel bound, against rounding in positions,
/// clamped speeds and `Δt`. It is far above F64 rounding and far below any
/// meaningful distance.
pub const ROUNDING_MARGIN: f64 = 1.0 / 1_048_576.0;

/// The interval between two epochs of one world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gap {
    /// Committed ticks between them.
    pub ticks: u64,
    /// The farthest any organism can have moved along either axis:
    /// `ticks · Δt · maximum speed`.
    pub travel_bound: f64,
    /// Whether that bound, with [`ROUNDING_MARGIN`], is under half of each
    /// extent, so the minimum wrapped displacement is the true one.
    pub unambiguous: bool,
}

impl Gap {
    /// The gap from `prior` to `current`.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when the epochs come from worlds with a
    /// different extent, timestep or speed limit, or `current` is not
    /// later.
    pub fn between(prior: &Epoch, current: &Epoch) -> Result<Self, CoreError> {
        let conflict = |detail: String| CoreError::StateConflict {
            operation: "measure render epoch gap",
            detail,
        };
        if prior.extent() != current.extent()
            || prior.step() != current.step()
            || prior.maximum_speed().to_bits() != current.maximum_speed().to_bits()
        {
            return Err(conflict(format!(
                "epochs {} and {} come from different worlds",
                prior.id(),
                current.id()
            )));
        }
        let ticks = current
            .tick()
            .get()
            .checked_sub(prior.tick().get())
            .filter(|&ticks| ticks > 0)
            .ok_or_else(|| {
                conflict(format!(
                    "tick {} is not after tick {}",
                    current.tick().get(),
                    prior.tick().get()
                ))
            })?;
        let travel_bound =
            ticks as f64 * current.step().seconds_per_tick() * current.maximum_speed();
        let [width, height] = current.extent();
        let reach = 2.0 * travel_bound * (1.0 + ROUNDING_MARGIN);
        Ok(Self {
            ticks,
            travel_bound,
            unambiguous: reach < width && reach < height,
        })
    }
}

/// One identity's fate between two epochs, by row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    /// Alive in both.
    Continuing {
        /// Its row in the earlier epoch.
        prior: usize,
        /// Its row in the later epoch.
        current: usize,
    },
    /// Born after the earlier epoch.
    Born {
        /// Its row in the later epoch.
        current: usize,
    },
    /// Dead by the later epoch.
    Died {
        /// Its row in the earlier epoch.
        prior: usize,
    },
}

/// The merge of two epochs' identities, in identity order.
#[derive(Debug, Clone)]
pub struct Pairs<'a> {
    prior: &'a [u64],
    current: &'a [u64],
    i: usize,
    j: usize,
}

impl Iterator for Pairs<'_> {
    type Item = Match;

    fn next(&mut self) -> Option<Match> {
        let (i, j) = (self.i, self.j);
        let found = match (self.prior.get(i), self.current.get(j)) {
            (Some(a), Some(b)) if a == b => Match::Continuing {
                prior: i,
                current: j,
            },
            (Some(a), Some(b)) if a < b => Match::Died { prior: i },
            (Some(_), None) => Match::Died { prior: i },
            (_, Some(_)) => Match::Born { current: j },
            (None, None) => return None,
        };
        match found {
            Match::Continuing { .. } => {
                self.i += 1;
                self.j += 1;
            }
            Match::Died { .. } => self.i += 1,
            Match::Born { .. } => self.j += 1,
        }
        Some(found)
    }
}

/// Match `prior`'s organisms with `current`'s by identity.
#[must_use]
pub fn pairs<'a>(prior: &'a Epoch, current: &'a Epoch) -> Pairs<'a> {
    Pairs {
        prior: prior.ids(),
        current: current.ids(),
        i: 0,
        j: 0,
    }
}

/// `from` moved `fraction` of the way to `to` along a periodic axis of
/// `length` by the minimum wrapped displacement, wrapped into
/// `[0, length)`. It is `from` at fraction 0 and `to` at fraction 1, and
/// the true path in between only when the true displacement is under half
/// of `length`, which a [`Gap`] decides.
#[must_use]
pub fn wrapped_between(from: f64, to: f64, length: f64, fraction: f64) -> f64 {
    if fraction <= 0.0 {
        return from;
    }
    if fraction >= 1.0 {
        return to;
    }
    let mut displacement = to - from;
    if displacement >= length / 2.0 {
        displacement -= length;
    } else if displacement < -length / 2.0 {
        displacement += length;
    }
    let wrapped = (from + fraction * displacement).rem_euclid(length);
    // A tiny negative value wraps to `length` itself in floating point.
    if wrapped >= length { 0.0 } else { wrapped }
}

/// Where an epoch's organisms are drawn between two committed epochs (plan
/// §13.5): one-state display latency, the display moving from `prior` to
/// `current`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Interpolation {
    /// Per row of `current`: where its organism is drawn.
    pub positions: Vec<[f64; 2]>,
    /// Per row of `current`: whether it is drawn yet. An organism born in
    /// the gap appears with the epoch that first shows it, never before
    /// its birth.
    pub shown: Vec<bool>,
    /// Rows of `prior` whose organisms died in the gap: drawn, if at all,
    /// at their `prior` positions and fading (they are dead scientifically
    /// from the death on).
    pub fading: Vec<usize>,
    /// Whether the gap was ambiguous: every continuing organism snapped to
    /// `current`, and trails must reset.
    pub snapped: bool,
}

/// Fill `into` with where `current`'s organisms are drawn `fraction` of the
/// way from `prior` to `current`, matched by identity. A continuing
/// organism moves by the minimum wrapped displacement when the [`Gap`] is
/// unambiguous, and snaps to `current` otherwise, so no invented reverse
/// path is ever drawn. At fraction 0 every continuing organism is at its
/// `prior` position and at fraction 1 at its `current` one, exactly.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a fraction outside `[0, 1]`; as
/// [`Gap::between`]; [`CoreError::AllocationFailed`] when `into` cannot
/// grow. `into` is cleared first.
pub fn interpolate(
    prior: &Epoch,
    current: &Epoch,
    fraction: f64,
    into: &mut Interpolation,
) -> Result<Gap, CoreError> {
    if !(0.0..=1.0).contains(&fraction) {
        return Err(CoreError::NumericalInvalid {
            quantity: "interpolation fraction",
            detail: format!("{fraction} is outside [0, 1]"),
        });
    }
    let gap = Gap::between(prior, current)?;
    into.positions.clear();
    into.shown.clear();
    into.fading.clear();
    into.snapped = false;
    let rows = current.len();
    let grow = |buffer: &'static str, bytes: usize| CoreError::AllocationFailed {
        buffer,
        bytes: u64::try_from(rows.saturating_mul(bytes)).unwrap_or(u64::MAX),
    };
    into.positions
        .try_reserve_exact(rows)
        .map_err(|_| grow("interpolated positions", size_of::<[f64; 2]>()))?;
    into.shown
        .try_reserve_exact(rows)
        .map_err(|_| grow("interpolated visibility", size_of::<bool>()))?;
    let [width, height] = current.extent();
    let (from, to) = (prior.positions(), current.positions());
    for found in pairs(prior, current) {
        match found {
            Match::Continuing {
                prior: i,
                current: j,
            } => {
                let (a, b) = (from[i].map(f64::from), to[j].map(f64::from));
                if gap.unambiguous {
                    into.positions.push([
                        wrapped_between(a[0], b[0], width, fraction),
                        wrapped_between(a[1], b[1], height, fraction),
                    ]);
                } else {
                    into.snapped = true;
                    into.positions.push(b);
                }
                into.shown.push(true);
            }
            Match::Born { current: j } => {
                into.positions.push(to[j].map(f64::from));
                into.shown.push(fraction >= 1.0);
            }
            Match::Died { prior: i } => {
                into.fading
                    .try_reserve(1)
                    .map_err(|_| grow("fading rows", size_of::<usize>()))?;
                into.fading.push(i);
            }
        }
    }
    Ok(gap)
}
