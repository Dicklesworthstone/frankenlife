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
//! `InjectMaterial` is the material brush (§14.3): signed chemical material
//! with declared energy. Its direction `a` has `||a||₂ ≤ 1` (squares summed
//! in channel order, the rule for a source's direction). The cell's energy
//! is raised exactly as `InjectEnergy` raises it, to the change `Δe`
//! actually made, and then each moment channel `k` gains `Δe · a_k`, rounded
//! to the nearest `f32`, in channel order. The packet `(Δe, Δe a)` is
//! admissible (`||Δe a||₂ ≤ Δe`), so it can never write an arbitrary `m`
//! that breaks `||m||₂ ≤ e` beyond the field's rounding tolerance. Its
//! `command-injection` flow is `Δe`.
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
use flife_field::sources::squared_norm;
use flife_field::state::CHANNELS;
use flife_field::{Deposits, FieldGrid, FieldState};

use crate::lifecycle::{Death, Disposition, Recycling};
use crate::population::{Organism, Population};

/// The components of a material brush's direction: one per moment channel
/// of the field.
pub const MATERIAL_CHANNELS: usize = CHANNELS;

/// A validated change to the world, effective at one tick boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "a material's 32 components keep the intervention `Copy` and unboxed; a tick seals a handful"
)]
pub enum Intervention {
    /// Declared external energy (`ecological-energy`) added to the field
    /// cell at `cell = [x, y]`, recorded as `command-injection`.
    InjectEnergy {
        /// The cell's `[x, y]` index on the field grid.
        cell: [u32; 2],
        /// Energy to add: finite, strictly positive and at most `f32::MAX`.
        amount: f64,
    },
    /// Declared external energy (`ecological-energy`) added to the field
    /// cell at `cell = [x, y]` with signed chemical composition `direction`:
    /// the material brush. Recorded as `command-injection`.
    InjectMaterial {
        /// The cell's `[x, y]` index on the field grid.
        cell: [u32; 2],
        /// Energy to add: finite, strictly positive and at most `f32::MAX`.
        amount: f64,
        /// The composition per unit energy: finite, `||direction||₂ ≤ 1`.
        direction: [f64; MATERIAL_CHANNELS],
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
    /// `f32::MAX`; for a material direction with a nonfinite component or
    /// a norm above one; for a centre outside the world or a radius that is
    /// not finite, not strictly positive, or above half the shorter world
    /// side.
    pub fn check(&self, grid: &FieldGrid) -> Result<(), CoreError> {
        match *self {
            Self::InjectEnergy { cell, amount } => check_injection(grid, cell, amount),
            Self::InjectMaterial {
                cell,
                amount,
                ref direction,
            } => {
                check_injection(grid, cell, amount)?;
                // A nonfinite component makes the sum of squares NaN or
                // infinite, outside the interval, so this one test refuses it.
                let norm_squared = squared_norm(direction);
                if !(0.0..=1.0).contains(&norm_squared) {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "material direction",
                        detail: format!(
                            "||a||² = {norm_squared:?}: the components must be finite and \
                             the norm at most 1"
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
            Self::InjectEnergy { amount, .. } | Self::InjectMaterial { amount, .. } => amount,
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
            Self::InjectEnergy { .. } | Self::InjectMaterial { .. } => Ok(false),
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
            Intervention::InjectEnergy { cell, amount } => inject_energy(field, cell, amount)?.1,
            Intervention::InjectMaterial {
                cell,
                amount,
                ref direction,
            } => {
                let (index, energy) = inject_energy(field, cell, amount)?;
                for (channel, &component) in (0_u32..).zip(direction.iter()) {
                    if component == 0.0 {
                        continue;
                    }
                    let plane =
                        field
                            .moment_mut(channel)
                            .ok_or_else(|| CoreError::LayoutMismatch {
                                buffer: "field",
                                detail: format!("moment channel {channel} is missing"),
                            })?;
                    let before = plane[index];
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "the moment is stored as the nearest f32, as a source deposits it"
                    )]
                    let after = (f64::from(before) + energy * component) as f32;
                    if !after.is_finite() {
                        return Err(CoreError::NumericalInvalid {
                            quantity: "injected cell moment",
                            detail: format!("channel {channel} at {cell:?} would hold {after:e}"),
                        });
                    }
                    plane[index] = after;
                }
                energy
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

/// An injection's cell and amount, as [`Intervention::check`] requires them.
fn check_injection(grid: &FieldGrid, cell: [u32; 2], amount: f64) -> Result<(), CoreError> {
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
            detail: format!("{amount:e} is not a finite positive energy at most f32::MAX"),
        });
    }
    Ok(())
}

/// Raise `cell`'s energy by `amount`, rounded down to the largest `f32` not
/// above `e + amount`: the cell's index and the energy actually added.
fn inject_energy(
    field: &mut FieldState,
    cell: [u32; 2],
    amount: f64,
) -> Result<(usize, f64), CoreError> {
    let index =
        field
            .grid()
            .cell_index(cell[0], cell[1])
            .ok_or_else(|| CoreError::NumericalInvalid {
                quantity: "injection cell",
                detail: format!("{cell:?} is outside the grid"),
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
            detail: format!("{amount:e} added to {before:e} at {cell:?} is not a finite f32"),
        });
    }
    plane[index] = after;
    Ok((index, f64::from(after) - f64::from(before)))
}

#[cfg(test)]
mod tests {
    //! The material brush's arithmetic on a bare field, where nothing else
    //! touches the cell: the oracle is the module documentation's packet,
    //! written out as the expected `f32` values.

    use flife_core::{Flow, Holdings, ResidualBudget, Tick, TickClosure, TickLedger};
    use flife_field::{BufferLimits, ConeTolerance, FieldGrid, FieldState};

    use super::{AppliedIntervention, Intervention, MATERIAL_CHANNELS, apply};

    /// A cell of the 4 × 3 grid other than the first.
    const CELL: [u32; 2] = [2, 1];

    fn field() -> FieldState {
        let grid = FieldGrid::new(4, 3, 4.0, 3.0).expect("grid");
        FieldState::zeroed(grid, &BufferLimits::host()).expect("field")
    }

    fn index(field: &FieldState) -> usize {
        field.grid().cell_index(CELL[0], CELL[1]).expect("cell")
    }

    fn moment(field: &FieldState, channel: u32) -> f32 {
        field.moment(channel).expect("channel")[index(field)]
    }

    fn set_moment(field: &mut FieldState, channel: u32, value: f32) {
        let at = index(field);
        field.moment_mut(channel).expect("channel")[at] = value;
    }

    fn material(amount: f64, entries: &[(usize, f64)]) -> Intervention {
        let mut direction = [0.0; MATERIAL_CHANNELS];
        for &(channel, value) in entries {
            direction[channel] = value;
        }
        Intervention::InjectMaterial {
            cell: CELL,
            amount,
            direction,
        }
    }

    fn total(field: &FieldState) -> f64 {
        field.energy().iter().map(|&energy| f64::from(energy)).sum()
    }

    /// `interventions` applied at tick 0, closed exactly against the field's
    /// energy before and after.
    fn applied(
        field: &mut FieldState,
        interventions: &[Intervention],
    ) -> (Vec<AppliedIntervention>, TickClosure) {
        let opening = Holdings::new(total(field), 0.0).expect("holdings");
        let mut ledger = TickLedger::open(Tick::new(0), opening, ResidualBudget::EXACT);
        let mut applied = Vec::new();
        apply(
            Tick::new(0),
            interventions,
            field,
            &mut ledger,
            &mut applied,
        )
        .expect("applies");
        let closing = Holdings::new(total(field), 0.0).expect("holdings");
        (applied, ledger.close(closing).expect("closes exactly"))
    }

    /// Each named channel gains `Δe · a_k` rounded to the nearest `f32`;
    /// unnamed channels and every other cell are untouched, a `-0.0` moment
    /// included; the flow is `Δe`.
    #[test]
    fn the_packet_is_the_energy_added_times_the_direction() {
        let mut field = field();
        let at = index(&field);
        field.energy_mut()[at] = 2.0;
        field.energy_mut()[0] = 9.0;
        set_moment(&mut field, 0, 0.5);
        set_moment(&mut field, 3, -0.25);
        set_moment(&mut field, 5, 0.125);
        set_moment(&mut field, 6, -0.0);
        let mut expected = field.clone();
        let (applied, closure) = applied(&mut field, &[material(3.0, &[(0, 0.6), (3, -0.8)])]);

        expected.energy_mut()[at] = 5.0;
        set_moment(&mut expected, 0, 2.3);
        set_moment(&mut expected, 3, -2.65);
        let bits = |field: &FieldState| -> Vec<u32> {
            (0..flife_field::FIELD_PLANES)
                .flat_map(|plane| field.plane(plane).expect("plane").to_vec())
                .map(f32::to_bits)
                .collect()
        };
        assert_eq!(bits(&field), bits(&expected));
        assert_eq!(moment(&field, 6).to_bits(), (-0.0_f32).to_bits());
        let [record] = applied.as_slice() else {
            panic!("one record: {applied:?}");
        };
        assert_eq!(
            (record.position, record.applied, record.removed),
            (0, 3.0, 0)
        );
        assert_eq!(closure.flow(Flow::CommandInjection), 3.0);
    }

    /// When the cell rounds the energy down, the composition is for the
    /// energy actually added, not the energy declared.
    #[test]
    fn a_rounded_down_injection_carries_composition_for_what_it_added() {
        let mut field = field();
        let at = index(&field);
        field.energy_mut()[at] = 1.0;
        let (applied, closure) = applied(&mut field, &[material(0.1, &[(0, 1.0)])]);
        // 1.1 rounds to 1.100_000_02 in f32, above the target, so the cell
        // takes the f32 below it; the difference is exact.
        let after = 1.1_f32.next_down();
        assert_eq!(field.energy()[at], after);
        let added = after - 1.0;
        assert_eq!(applied[0].applied, f64::from(added));
        assert_eq!(closure.flow(Flow::CommandInjection), f64::from(added));
        assert_eq!(moment(&field, 0), added);
        assert!(moment(&field, 0) < 0.1);
    }

    /// Unit-norm material keeps the cone: into an empty cell, and onto a
    /// cell already on the cone along the same direction.
    #[test]
    fn unit_material_keeps_the_composition_admissible() {
        let mut field = field();
        let at = index(&field);
        applied(&mut field, &[material(7.3, &[(0, 0.6), (1, 0.8)])]);
        let energy = f64::from(field.energy()[at]);
        let norm = f64::from(moment(&field, 0)).hypot(f64::from(moment(&field, 1)));
        assert!(energy > 7.29, "{energy}");
        assert!(
            norm <= energy * (1.0 + f64::from(f32::EPSILON)),
            "{norm} > {energy}"
        );
        let tolerance = ConeTolerance::new(0.0, 2.0 * f64::from(f32::EPSILON)).expect("tolerance");
        field.check_invariants(tolerance).expect("admissible");

        let mut field = self::field();
        field.energy_mut()[at] = 4.0;
        set_moment(&mut field, 2, -4.0);
        applied(&mut field, &[material(2.0, &[(2, -1.0)])]);
        assert_eq!(field.energy()[at], 6.0);
        assert_eq!(moment(&field, 2), -6.0);
        field
            .check_invariants(ConeTolerance::EXACT)
            .expect("on the cone");
    }
}
