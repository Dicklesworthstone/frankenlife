//! Scientific interventions applied at a tick boundary (plan §8.1 phase 1,
//! §12.3, §14.3; FL-014).
//!
//! An [`Intervention`] is the canonical, world-space form of an accepted
//! simulation-changing command. The session owner seals the interventions
//! effective at tick `n` in their admission order and hands them to
//! [`World::advance_with`](crate::World::advance_with). A removal applies
//! first, at the boundary itself (phase 1); an injection applies with phase
//! 2's external sources, after transport, into the tick's observation field,
//! so every organism senses it during tick `n`. The published tick reports
//! every one in its sealed position.
//!
//! `InjectEnergy` adds energy to one cell's energy plane and leaves its
//! moments unchanged. Raising `e` with `m` fixed keeps `||m||₂ ≤ e` (INV-02),
//! so the result is admissible composition. The cell is rounded down to the
//! largest `f32` not above `e + amount`, so the field never gains more than
//! was declared. The recorded `command-injection` flow is the change actually
//! made, which can be less than the request by at most one `f32` ulp of the
//! cell.
//!
//! `RemoveOrganisms` takes out every organism whose committed position lies
//! in a disk on the torus: within `radius` of `center` by the minimum-image
//! distance, the circle included (plan §7.6's explicit removal, §12.3's
//! population removal, §14.3's particle removal). Removed organisms are
//! gone before phase 2: they sense nothing, ask for no energy, pay nothing
//! and leave no offspring in tick `n`. Each dies once, at its committed
//! position, with its whole energy as the residual and the command's
//! [`Disposition`] as its cause: removed from the modeled world
//! (`command-removal`), dissipated (`unrecycled-death`), or recycled, where
//! the configured fraction is offered to the field in phase 9 like any
//! recycled residual and is first sensed in tick `n + 1`. An organism inside
//! two removals' disks at one boundary dies by the first in sealed order. A
//! disk that holds no organism removes nothing and still applies.

use flife_core::ledger::{Flow, TickLedger};
use flife_core::{CoreError, Tick};
use flife_field::{Deposits, FieldGrid, FieldState};

use crate::lifecycle::{Death, Disposition, Recycling};
use crate::population::{Organism, Population};

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
    /// Every organism within `radius` world units of `center` on the torus
    /// dies at the boundary, removed with `disposition`.
    RemoveOrganisms {
        /// The disk's centre: a world position in `[0, L_x) × [0, L_y)`.
        center: [f64; 2],
        /// Its radius in world units: finite, strictly positive and at most
        /// half the shorter world side, so the disk never overlaps itself.
        radius: f64,
        /// What becomes of each removed organism's energy.
        disposition: Disposition,
    },
}

impl Intervention {
    /// Check the intervention against `grid` before it is admitted.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a cell outside the grid or an
    /// amount that is not finite, not strictly positive, or above
    /// `f32::MAX`; for a centre outside the world or a radius that is not
    /// finite, not strictly positive, or above half the shorter world side.
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
            Self::RemoveOrganisms { center, radius, .. } => {
                let (width, height) = (grid.world_width(), grid.world_height());
                if !(center[0].is_finite()
                    && center[1].is_finite()
                    && (0.0..width).contains(&center[0])
                    && (0.0..height).contains(&center[1]))
                {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "removal centre",
                        detail: format!(
                            "{center:?} is not a world position in [0, {width}) × [0, {height})"
                        ),
                    });
                }
                let limit = 0.5 * width.min(height);
                if !(radius.is_finite() && radius > 0.0 && radius <= limit) {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "removal radius",
                        detail: format!(
                            "{radius:e} is not a finite positive radius at most {limit}, \
                             half the shorter world side"
                        ),
                    });
                }
                Ok(())
            }
        }
    }

    /// The energy the intervention declares it adds: none for a removal.
    #[must_use]
    pub const fn declared_energy(&self) -> f64 {
        match *self {
            Self::InjectEnergy { amount, .. } => amount,
            Self::RemoveOrganisms { .. } => 0.0,
        }
    }

    /// Whether `position`, a wrapped world position, lies in this removal's
    /// disk: the minimum-image distance from the centre is at most the
    /// radius. Never for an injection.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a position outside the world.
    pub fn removes_at(&self, grid: &FieldGrid, position: [f64; 2]) -> Result<bool, CoreError> {
        match *self {
            Self::InjectEnergy { .. } => Ok(false),
            Self::RemoveOrganisms { center, radius, .. } => {
                let [dx, dy] = grid.displacement(center, position)?;
                Ok(dx * dx + dy * dy <= radius * radius)
            }
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
    /// The energy it moved: for an injection, the energy that entered the
    /// field, never more than declared; for a removal, the energy the
    /// organisms it removed held.
    pub applied: f64,
    /// The organisms it removed: none for an injection.
    pub removed: u64,
}

/// Phase 1: take out the organisms `interventions`' removals name, in
/// sealed order, from `rows`, the committed population in identity order.
/// Each removed organism's death is appended to `deaths` (sealed order,
/// then identity order) and settled under `recycling`, its residual
/// recorded in `ledger` or staged in `deposits`; each removal is appended to
/// `applied`. `rows` keeps the rest, still in identity order.
#[allow(
    clippy::too_many_arguments,
    reason = "the phase's inputs and the four outputs it fills"
)]
pub(crate) fn remove(
    tick: Tick,
    interventions: &[Intervention],
    grid: &FieldGrid,
    population: &Population,
    recycling: &Recycling,
    rows: &mut Vec<Organism>,
    deaths: &mut Vec<Death>,
    deposits: &mut Deposits,
    ledger: &mut TickLedger,
    applied: &mut Vec<AppliedIntervention>,
) -> Result<(), CoreError> {
    let to = tick.next()?;
    let mut gone = vec![false; rows.len()];
    for (position, intervention) in interventions.iter().enumerate() {
        let Intervention::RemoveOrganisms { disposition, .. } = *intervention else {
            continue;
        };
        intervention.check(grid)?;
        let mut energy = 0.0;
        let mut removed = 0_u64;
        for (organism, gone) in rows.iter().zip(gone.iter_mut()) {
            if *gone || !intervention.removes_at(grid, organism.position)? {
                continue;
            }
            *gone = true;
            let death = Death::removed(
                organism,
                to,
                organism.position,
                organism.energy,
                disposition,
            )?;
            let genome =
                population
                    .genome(organism.id)
                    .ok_or_else(|| CoreError::StateConflict {
                        operation: "remove organism",
                        detail: format!("{} has no genome", organism.id),
                    })?;
            recycling
                .settle(grid, &death, genome.traits().affinity(), deposits)?
                .record(ledger)?;
            deaths.push(death);
            energy += organism.energy;
            removed += 1;
        }
        applied.push(AppliedIntervention {
            tick,
            position,
            intervention: *intervention,
            applied: energy,
            removed,
        });
    }
    let mut kept = gone.iter();
    rows.retain(|_| kept.next().is_some_and(|gone| !gone));
    Ok(())
}

/// Apply `interventions`' injections in order into `field`, the tick's
/// observation buffer, recording each in `ledger` and appending it to
/// `applied`.
pub(crate) fn apply(
    tick: Tick,
    interventions: &[Intervention],
    field: &mut FieldState,
    ledger: &mut TickLedger,
    applied: &mut Vec<AppliedIntervention>,
) -> Result<(), CoreError> {
    for (position, intervention) in interventions.iter().enumerate() {
        if matches!(intervention, Intervention::RemoveOrganisms { .. }) {
            continue;
        }
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
            Intervention::RemoveOrganisms { .. } => continue,
        };
        ledger.record(Flow::CommandInjection, energy)?;
        applied.push(AppliedIntervention {
            tick,
            position,
            intervention: *intervention,
            applied: energy,
            removed: 0,
        });
    }
    Ok(())
}
