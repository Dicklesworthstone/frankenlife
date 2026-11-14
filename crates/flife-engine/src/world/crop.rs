//! Local packages (FL-042; plan §14.5): one disk of a committed world, with
//! everything outside it emptied.
//!
//! [`World::cropped`] keeps the world's configuration, tick, resource
//! epoch, event count, source paths and identity allocators, and of its
//! matter only what one disk holds, as [`World::measure_region`] reads it:
//!
//! - the organisms a removal of the disk would take, each with its whole
//!   row and held state, and its genome rebuilt through a fresh quota and
//!   seal as a checkpoint's restore rebuilds it;
//! - the field cells whose centres lie in the disk, every plane (energy and
//!   moments) copied. Every other cell is zero.
//!
//! Its ledger starts afresh: its initial and current holdings are the
//! cropped field's and organisms' energy (the package's endowment), and no
//! tick is recorded. A checkpoint of it restores to the same world, digest
//! for digest, so the checkpoint is the package's verified payload. What the
//! package declares about its boundary is the command line's: the crop is a
//! world only so that the checkpoint encoding carries it.

use flife_core::ids::GenomeId;
use flife_core::{CoreError, EnergyLedger, Holdings};
use flife_field::{BufferLimits, FieldPair};
use flife_genome::{Genome, GenomeQuota, Traits, WeightsMut};
use flife_neural::Weights;

use super::{
    Controllers, Fields, Rules, Scratch, Sensors, Settle, Uptake, Watch, World, initial_sources,
    organism_total,
};
use crate::intervention::Intervention;
use crate::lifecycle::Disposition;
use crate::measure::{RegionMeasurement, cell_in_disk};
use crate::population::{Entrant, Population};

fn refuse(detail: impl Into<String>) -> CoreError {
    CoreError::StateConflict {
        operation: "crop world",
        detail: detail.into(),
    }
}

/// `genome`'s topology, traits and parameters as genome `id` with `parent`,
/// through `quota`'s allocation and `rules`' seal; its traits are checked
/// again against `rules`' limits.
pub(super) fn reseal(
    quota: &mut GenomeQuota,
    rules: &Rules,
    genome: &Genome,
    id: GenomeId,
    parent: Option<GenomeId>,
) -> Result<Genome, CoreError> {
    let traits = genome.traits();
    let traits = Traits::new(
        *traits.affinity(),
        *traits.motion_response(),
        traits.sensing_scale(),
        &rules.limits,
    )?;
    let mut buffer = quota.allocate(&rules.catalog, genome.topology())?;
    let copied = match (buffer.values_mut(), genome.parameters()) {
        (WeightsMut::F32(to), Weights::F32(from)) if to.len() == from.len() => {
            to.copy_from_slice(from);
            true
        }
        (WeightsMut::F16(to), Weights::F16(from)) if to.len() == from.len() => {
            to.copy_from_slice(from);
            true
        }
        _ => false,
    };
    if !copied {
        quota.abandon(buffer)?;
        return Err(refuse(format!(
            "{}'s parameters do not fit its topology's buffer",
            genome.id()
        )));
    }
    Genome::seal(buffer, id, &rules.catalog, traits, parent, &rules.limits).or_else(|unsealed| {
        quota.abandon(unsealed.buffer)?;
        Err(unsealed.error)
    })
}

impl World {
    /// The committed world with everything outside the disk of `radius`
    /// world units about `center` emptied, and that disk's measurement. It
    /// keeps the organisms a removal of the disk would take, their genomes
    /// rebuilt through a fresh quota, and every plane of the field cells
    /// whose centres lie in the disk; every other cell is zero. Its ledger
    /// opens at those holdings.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a disk a measurement refuses;
    /// [`CoreError::StateConflict`] for a world a fault stopped; and the
    /// errors of the field's, quota's and population's allocation.
    pub fn cropped(
        &self,
        center: [f64; 2],
        radius: f64,
    ) -> Result<(Self, RegionMeasurement), CoreError> {
        if let Some(fault) = &self.fault {
            return Err(refuse(format!("the run stopped: {fault:?}")));
        }
        let measurement = self.measure_region(center, radius)?;
        let rules = Rules::from_config(&self.config, self.rules.profile)?;
        let grid = rules.grid;

        let mut field = FieldPair::zeroed(grid, &BufferLimits::host())?;
        let source = self.field.current();
        for y in 0..grid.cells_y() {
            for x in 0..grid.cells_x() {
                if !cell_in_disk(&grid, center, radius, x, y)? {
                    continue;
                }
                let cell = source
                    .cell(x, y)
                    .ok_or_else(|| refuse(format!("the field holds no cell ({x}, {y})")))?;
                field
                    .current_mut()
                    .set_cell(x, y, &cell)
                    .ok_or_else(|| refuse(format!("the crop holds no cell ({x}, {y})")))?;
            }
        }

        let disk = Intervention::RemoveOrganisms {
            center,
            radius,
            disposition: Disposition::Remove,
        };
        let mut quota = GenomeQuota::from_config(&self.config)?;
        let mut entrants = Vec::new();
        for (organism, genome) in self.population.iter_with_genomes() {
            if !disk.removes_at(&grid, organism.position)? {
                continue;
            }
            let genome = genome.ok_or_else(|| refuse(format!("{} has no genome", organism.id)))?;
            let genome = match reseal(&mut quota, &rules, genome, genome.id(), genome.parent()) {
                Ok(genome) => genome,
                Err(error) => {
                    for entrant in entrants {
                        let Entrant { genome, .. } = entrant;
                        quota.retire(genome)?;
                    }
                    return Err(error);
                }
            };
            entrants.push(Entrant { organism, genome });
        }
        let mut population = Population::from_config(self.population.arrangement(), &self.config)?;
        if let Err(error) = population.admit(&mut entrants) {
            for entrant in entrants {
                let Entrant { genome, .. } = entrant;
                quota.retire(genome)?;
            }
            return Err(error);
        }

        let holdings = Holdings::new(field.current().energy_total(), organism_total(&population))?;
        let ledger = EnergyLedger::new(holdings, rules.profile.residual);
        let sources = initial_sources(&self.config, &rules)?;
        let scratch = Scratch::new(&self.config, &rules, sources.len())?;
        Ok((
            Self {
                config: self.config.clone(),
                rules,
                tick: self.tick,
                epoch: self.epoch,
                events: self.events,
                field,
                population,
                sources,
                paths: self.paths.clone(),
                identities: self.identities.clone(),
                quota,
                ledger,
                fault: None,
                scratch,
                controllers: Controllers::default(),
                fields: Fields::default(),
                sensors: Sensors::default(),
                uptake: Uptake::default(),
                settle: Settle::default(),
                watch: Watch::default(),
                ancestry: None,
            },
            measurement,
        ))
    }
}
