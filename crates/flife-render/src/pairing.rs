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
//! must snap to the later state (FL-020).

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
