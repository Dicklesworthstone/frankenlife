//! Scientific interventions applied at a tick boundary (plan §8.1 phase 1,
//! §12.3, §14.3; FL-014).
//!
//! An [`Intervention`] is the canonical, world-space form of an accepted
//! simulation-changing command. The session owner seals the interventions
//! effective at tick `n` in their admission order and hands them to
//! [`World::advance_with`](crate::World::advance_with). They apply with phase
//! 2's external sources, after transport, into the tick's observation field,
//! so every organism senses them during tick `n`. Each one is recorded in
//! the tick's ledger as the energy that actually entered the field.
//!
//! `InjectEnergy` adds energy to one cell's energy plane and leaves its
//! moments unchanged. Raising `e` with `m` fixed keeps `||m||₂ ≤ e` (INV-02),
//! so the result is admissible composition. The cell is rounded down to the
//! largest `f32` not above `e + amount`, so the field never gains more than
//! was declared. The recorded `command-injection` flow is the change actually
//! made, which can be less than the request by at most one `f32` ulp of the
//! cell.

use flife_core::ledger::{Flow, TickLedger};
use flife_core::{CoreError, Tick};
use flife_field::{FieldGrid, FieldState};

/// A validated change to the world, effective at one tick boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Intervention {
    /// Declared external energy (`ecological-energy`) added to the field
    /// cell at `cell = [x, y]`, recorded as `command-injection`.
    InjectEnergy {
        /// The cell's `[x, y]` index on the field grid.
        cell: [u32; 2],
        /// Energy to add: finite, strictly positive and at most `f32::MAX`.
        amount: f64,
    },
}

impl Intervention {
    /// Check the intervention against `grid` before it is admitted.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a cell outside the grid or an
    /// amount that is not finite, not strictly positive, or above
    /// `f32::MAX`.
    pub fn check(&self, grid: &FieldGrid) -> Result<(), CoreError> {
        match *self {
            Self::InjectEnergy { cell, amount } => {
                if grid.cell_index(cell[0], cell[1]).is_none() {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "injection cell",
                        detail: format!(
                            "{cell:?} is outside the {} × {} grid",
                            grid.cells_x(),
                            grid.cells_y()
                        ),
                    });
                }
                if !(amount.is_finite() && amount > 0.0 && amount <= f64::from(f32::MAX)) {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "injection amount",
                        detail: format!(
                            "{amount:e} is not a finite positive energy at most f32::MAX"
                        ),
                    });
                }
                Ok(())
            }
        }
    }

    /// The energy the intervention declares it adds.
    #[must_use]
    pub const fn declared_energy(&self) -> f64 {
        match *self {
            Self::InjectEnergy { amount, .. } => amount,
        }
    }
}

/// One intervention as a published tick applied it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppliedIntervention {
    /// The input tick `n` whose boundary it applied at.
    pub tick: Tick,
    /// Its position in that tick's sealed, admission-ordered set.
    pub position: usize,
    /// The intervention.
    pub intervention: Intervention,
    /// The energy that entered the field: never more than declared.
    pub applied: f64,
}

/// Apply `interventions` in order into `field`, the tick's observation
/// buffer, recording each in `ledger` and appending it to `applied`.
pub(crate) fn apply(
    tick: Tick,
    interventions: &[Intervention],
    field: &mut FieldState,
    ledger: &mut TickLedger,
    applied: &mut Vec<AppliedIntervention>,
) -> Result<(), CoreError> {
    for (position, intervention) in interventions.iter().enumerate() {
        intervention.check(field.grid())?;
        let energy = match *intervention {
            Intervention::InjectEnergy { cell, amount } => {
                let index = field.grid().cell_index(cell[0], cell[1]).ok_or_else(|| {
                    CoreError::NumericalInvalid {
                        quantity: "injection cell",
                        detail: format!("{cell:?} is outside the grid"),
                    }
                })?;
                let plane = field.energy_mut();
                let before = plane[index];
                let target = f64::from(before) + amount;
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "rounded to the nearest f32 and then down, never above the target"
                )]
                let mut after = target as f32;
                if f64::from(after) > target {
                    after = after.next_down();
                }
                if !after.is_finite() {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "injected cell energy",
                        detail: format!(
                            "{amount:e} added to {before:e} at {cell:?} is not a finite f32"
                        ),
                    });
                }
                plane[index] = after;
                f64::from(after) - f64::from(before)
            }
        };
        ledger.record(Flow::CommandInjection, energy)?;
        applied.push(AppliedIntervention {
            tick,
            position,
            intervention: *intervention,
            applied: energy,
        });
    }
    Ok(())
}
