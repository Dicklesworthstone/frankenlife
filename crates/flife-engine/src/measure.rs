//! A temporary measurement region (plan §14.3): what one disk of the
//! committed world holds, read between ticks for the Studio's measuring
//! tool.
//!
//! [`World::measure_region`](crate::World::measure_region) reads the disk
//! a removal of the same centre and radius would take
//! ([`Intervention::RemoveOrganisms`]), refused as that removal would be:
//!
//! - The organisms whose minimum-image distance from the centre is at most
//!   the radius, by the removal's own test ([`Intervention::removes_at`]).
//!   Their committed energies are summed in identity order, as the removal
//!   sums them, so a removal applied alone at this boundary removes exactly
//!   [`RegionMeasurement::organisms`], holding exactly
//!   [`RegionMeasurement::organism_energy`], bit for bit.
//! - The field cells whose centres lie in the disk, by the same distance
//!   test, and their committed energies summed in cell-index order. A cell
//!   counts whole or not at all, so the cells' area approximates the
//!   disk's only to within the cells along its edge.
//!
//! It reads each organism's row and the committed field's energy plane
//! once, copies no genome, and changes nothing: the world's digest is the
//! same after it. Its cost grows with the population and the grid, not
//! with the genome bank.

use flife_core::CoreError;
use flife_core::clock::Tick;
use flife_core::ids::LineageId;
use flife_field::{FieldGrid, FieldState};

use crate::intervention::Intervention;
use crate::lifecycle::Disposition;
use crate::population::Population;

/// What one disk of the committed world holds (plan §14.3's temporary
/// measurement region).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionMeasurement {
    /// The committed tick measured.
    pub tick: Tick,
    /// The disk's centre: a world position.
    pub center: [f64; 2],
    /// Its radius in world units.
    pub radius: f64,
    /// Live organisms in the disk: those a removal of it would take.
    pub organisms: u64,
    /// Their committed energies, summed in identity order.
    pub organism_energy: f64,
    /// Distinct founder lineages among them.
    pub lineages: u64,
    /// Field cells whose centres lie in the disk.
    pub cells: u64,
    /// Those cells' committed energies, summed in cell-index order.
    pub field_energy: f64,
}

/// Measure the disk of `radius` about `center` in the committed world at
/// `tick`: `population` and `field` on `grid`.
pub(crate) fn measure(
    tick: Tick,
    grid: &FieldGrid,
    population: &Population,
    field: &FieldState,
    center: [f64; 2],
    radius: f64,
) -> Result<RegionMeasurement, CoreError> {
    let disk = Intervention::RemoveOrganisms {
        center,
        radius,
        disposition: Disposition::Remove,
    };
    disk.check(grid).map_err(|error| match error {
        CoreError::NumericalInvalid { quantity, detail } => CoreError::NumericalInvalid {
            quantity: match quantity {
                "removal centre" => "measurement centre",
                "removal radius" => "measurement radius",
                other => other,
            },
            detail,
        },
        other => other,
    })?;

    let mut organisms = 0_u64;
    let mut organism_energy = 0.0;
    for organism in population.iter() {
        if disk.removes_at(grid, organism.position)? {
            organisms += 1;
            organism_energy += organism.energy;
        }
    }
    let lineages = distinct_lineages(grid, population, &disk, organisms)?;

    let energy = field.energy();
    let [width, height] = [grid.cell_width(), grid.cell_height()];
    let mut cells = 0_u64;
    let mut field_energy = 0.0;
    for y in 0..grid.cells_y() {
        let at_y = (f64::from(y) + 0.5) * height;
        for x in 0..grid.cells_x() {
            let at = [(f64::from(x) + 0.5) * width, at_y];
            let [dx, dy] = grid.displacement(center, at)?;
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            let value = grid
                .cell_index(x, y)
                .and_then(|index| energy.get(index))
                .ok_or_else(|| CoreError::StateConflict {
                    operation: "measure region",
                    detail: format!("the field holds no energy for cell ({x}, {y})"),
                })?;
            cells += 1;
            field_energy += f64::from(*value);
        }
    }

    Ok(RegionMeasurement {
        tick,
        center,
        radius,
        organisms,
        organism_energy,
        lineages,
        cells,
        field_energy,
    })
}

/// The distinct founder lineages of the `count` organisms `disk` takes.
fn distinct_lineages(
    grid: &FieldGrid,
    population: &Population,
    disk: &Intervention,
    count: u64,
) -> Result<u64, CoreError> {
    let refused = || CoreError::AllocationFailed {
        buffer: "measured lineages",
        bytes: count.saturating_mul(size_of::<LineageId>() as u64),
    };
    let mut lineages: Vec<LineageId> = Vec::new();
    lineages
        .try_reserve_exact(usize::try_from(count).map_err(|_| refused())?)
        .map_err(|_| refused())?;
    for organism in population.iter() {
        if disk.removes_at(grid, organism.position)? {
            lineages.push(organism.lineage);
        }
    }
    lineages.sort_unstable();
    lineages.dedup();
    Ok(u64::try_from(lineages.len()).unwrap_or(u64::MAX))
}
