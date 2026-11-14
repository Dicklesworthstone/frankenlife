//! Transplants (FL-042; plan §14.5): a local package grafted into a host
//! world as it is founded.
//!
//! [`World::graft`] takes a package's world (a [`World::cropped`] disk) and
//! puts its matter into a host at tick zero, before anything has been
//! published, so that everything the host then holds is its founding
//! endowment and no flow is needed to account for it:
//!
//! - The disk moves by a whole number of cells, `offset`, so cells map to
//!   cells. Each field cell whose centre lies in the package's disk, and
//!   each of its organisms, is placed at the host's disk centre plus its
//!   minimum-image displacement from the package's centre, wrapped on the
//!   host's torus. A host may be another size than the source world.
//! - The host cells those cells land on are replaced, every plane. The
//!   host's organisms in the host's disk are displaced: they are not in the
//!   grafted world.
//! - The package's organisms enter after the host's founders, in their
//!   identity order, each with the host's next organism and genome
//!   identities. They are founders: no parent, generation zero, born at
//!   tick zero, with an invalid held output, their stable phase and the
//!   founders' cooldown deadline. They keep their velocity, energy,
//!   genome topology, traits and parameters, and their kinship: the
//!   package's organisms of one lineage form one new lineage, named by its
//!   first member's new identity.
//! - Every genome, the host survivors' and the newcomers', is rebuilt in a
//!   fresh quota and sealed again under the host's limits, so a package
//!   organism whose traits or parameters the host's configuration does not
//!   allow refuses the graft.
//! - The ledger opens afresh at the grafted world's holdings.
//!
//! A graft is all or none: refused, it leaves the host as it was.

use std::collections::{BTreeMap, BTreeSet};

use flife_core::clock::{Tick, stable_phase};
use flife_core::ids::{Generation, LineageId, ResourceEpoch};
use flife_core::{CoreError, EnergyLedger, Holdings};
use flife_field::{BufferLimits, FieldPair};
use flife_genome::GenomeQuota;

use super::crop::reseal;
use super::{World, organism_total};
use crate::intervention::Intervention;
use crate::lifecycle::Disposition;
use crate::measure::cell_in_disk;
use crate::population::{Entrant, HeldOutput, Organism, Population};

fn refuse(detail: impl Into<String>) -> CoreError {
    CoreError::StateConflict {
        operation: "graft package",
        detail: detail.into(),
    }
}

/// What a graft did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Graft {
    /// The disk's centre in the host.
    pub center: [f64; 2],
    /// Its radius in world units.
    pub radius: f64,
    /// Whole cells the disk moved by, in x and y.
    pub offset: [i64; 2],
    /// Host cells replaced.
    pub cells: u64,
    /// The energy they held.
    pub displaced_field: f64,
    /// Host organisms displaced.
    pub displaced_organisms: u64,
    /// Their energy.
    pub displaced_organism_energy: f64,
    /// Organisms grafted.
    pub organisms: u64,
    /// The new lineages they form.
    pub lineages: u64,
    /// The host's holdings before the graft.
    pub before: Holdings,
    /// The package's: its endowment.
    pub endowment: Holdings,
    /// The grafted world's.
    pub after: Holdings,
}

impl World {
    /// Graft `package`, a crop of the disk of `radius` about `center`, into
    /// this founding world, moved by `offset` whole cells (see the [module
    /// documentation](self)).
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when this world is not a founding world
    /// (a tick published, a fault, a trace), the two do not share their cell
    /// size, topology catalog and parameter type, `package` holds an
    /// organism or a field value outside its disk, or two of its cells land
    /// on one host cell; [`CoreError::NumericalInvalid`] for a disk a
    /// measurement of either world refuses, or a package organism whose
    /// traits or parameters the host's limits refuse;
    /// [`CoreError::BudgetExhausted`] when the host's population or genome
    /// quota cannot take the grafted organisms; and the identity
    /// allocators' and phase's errors.
    #[allow(
        clippy::too_many_lines,
        reason = "one all-or-none transaction: checks, field, organisms, ledger"
    )]
    pub fn graft(
        &mut self,
        package: &Self,
        center: [f64; 2],
        radius: f64,
        offset: [i64; 2],
    ) -> Result<Graft, CoreError> {
        if self.tick != Tick::new(0)
            || self.epoch != ResourceEpoch::new(0)
            || self.events != 0
            || self.fault.is_some()
            || self.ancestry.is_some()
        {
            return Err(refuse(
                "only a founding world takes a graft: nothing published, no fault, no trace",
            ));
        }
        let (host, source) = (self.rules.grid, package.rules.grid);
        if [host.cell_width(), host.cell_height()].map(f64::to_bits)
            != [source.cell_width(), source.cell_height()].map(f64::to_bits)
        {
            return Err(refuse(format!(
                "the package's cells are {} × {}, the host's {} × {}",
                source.cell_width(),
                source.cell_height(),
                host.cell_width(),
                host.cell_height()
            )));
        }
        if self.rules.catalog != package.rules.catalog
            || self.quota.dtype() != package.quota.dtype()
        {
            return Err(refuse(
                "the package's genomes are of another topology catalog or parameter type",
            ));
        }
        package.measure_region(center, radius)?;
        let target = host.wrap_position([
            center[0] + offset[0] as f64 * host.cell_width(),
            center[1] + offset[1] as f64 * host.cell_height(),
        ])?;
        let measured = self.measure_region(target, radius)?;

        // The package is a crop of its disk: nothing outside it.
        let disk = Intervention::RemoveOrganisms {
            center,
            radius,
            disposition: Disposition::Remove,
        };
        for organism in package.population.iter() {
            if !disk.removes_at(&source, organism.position)? {
                return Err(refuse(format!(
                    "{} lies outside the package's disk",
                    organism.id
                )));
            }
        }

        // The field: each package cell in the disk to its host cell.
        let mut field = FieldPair::zeroed(host, &BufferLimits::host())?;
        field.current_mut().copy_from(self.field.current())?;
        let mut landed = BTreeSet::new();
        let mut displaced_field = 0.0;
        for y in 0..source.cells_y() {
            for x in 0..source.cells_x() {
                let cell = package
                    .field
                    .current()
                    .cell(x, y)
                    .ok_or_else(|| refuse(format!("the package holds no cell ({x}, {y})")))?;
                if !cell_in_disk(&source, center, radius, x, y)? {
                    if cell.energy != 0.0 || cell.moments.iter().any(|value| *value != 0.0) {
                        return Err(refuse(format!(
                            "the package's cell ({x}, {y}) lies outside its disk and is not empty"
                        )));
                    }
                    continue;
                }
                let at = [
                    (f64::from(x) + 0.5) * source.cell_width(),
                    (f64::from(y) + 0.5) * source.cell_height(),
                ];
                let [dx, dy] = source.displacement(center, at)?;
                let [hx, hy] = host.wrap_position([target[0] + dx, target[1] + dy])?;
                let (hx, hy) = (
                    (hx / host.cell_width()).floor() as i64,
                    (hy / host.cell_height()).floor() as i64,
                );
                let (hx, hy) = host.wrap_cell(hx, hy);
                if !landed.insert((hx, hy)) {
                    return Err(refuse(format!(
                        "two of the package's cells land on the host's cell ({hx}, {hy})"
                    )));
                }
                let replaced = field
                    .current()
                    .cell(hx, hy)
                    .ok_or_else(|| refuse(format!("the host holds no cell ({hx}, {hy})")))?;
                displaced_field += f64::from(replaced.energy);
                field
                    .current_mut()
                    .set_cell(hx, hy, &cell)
                    .ok_or_else(|| refuse(format!("the host holds no cell ({hx}, {hy})")))?;
            }
        }

        // The organisms: the host's outside the disk, then the package's.
        let host_disk = Intervention::RemoveOrganisms {
            center: target,
            radius,
            disposition: Disposition::Remove,
        };
        let mut quota = GenomeQuota::from_config(&self.config)?;
        let mut identities = self.identities.clone();
        let mut entrants = Vec::new();
        let mut lineages: BTreeMap<LineageId, LineageId> = BTreeMap::new();
        let cooldown_deadline = Tick::new(0).checked_add(self.config.report().cooldown_ticks)?;
        let built = (|| {
            for (organism, genome) in self.population.iter_with_genomes() {
                if host_disk.removes_at(&host, organism.position)? {
                    continue;
                }
                let genome =
                    genome.ok_or_else(|| refuse(format!("{} has no genome", organism.id)))?;
                let genome = reseal(
                    &mut quota,
                    &self.rules,
                    genome,
                    genome.id(),
                    genome.parent(),
                )?;
                entrants.push(Entrant { organism, genome });
            }
            for (organism, genome) in package.population.iter_with_genomes() {
                let genome =
                    genome.ok_or_else(|| refuse(format!("{} has no genome", organism.id)))?;
                let id = identities.organisms.issue()?;
                let genome_id = identities.genomes.issue()?;
                let genome = reseal(&mut quota, &self.rules, genome, genome_id, None)?;
                let lineage = *lineages
                    .entry(organism.lineage)
                    .or_insert_with(|| LineageId::new(id.get()));
                let [dx, dy] = source.displacement(center, organism.position)?;
                entrants.push(Entrant {
                    organism: Organism {
                        id,
                        genome: genome_id,
                        parent: None,
                        lineage,
                        generation: Generation::FOUNDER,
                        birth_tick: Tick::new(0),
                        position: host.wrap_position([target[0] + dx, target[1] + dy])?,
                        velocity: organism.velocity,
                        energy: organism.energy,
                        energy_change_rate: 0.0,
                        held: HeldOutput::INVALID,
                        phase: stable_phase(id, self.rules.interval)?,
                        cooldown_deadline,
                    },
                    genome,
                });
            }
            let mut population =
                Population::from_config(self.population.arrangement(), &self.config)?;
            population.admit(&mut entrants)?;
            Ok::<_, CoreError>(population)
        })();
        let population = match built {
            Ok(population) => population,
            Err(error) => {
                for entrant in entrants {
                    let Entrant { genome, .. } = entrant;
                    quota.retire(genome)?;
                }
                return Err(error);
            }
        };

        let before = self.ledger.holdings();
        let endowment = package.ledger.holdings();
        let after = Holdings::new(field.current().energy_total(), organism_total(&population))?;
        let grafted = u64::try_from(package.population.len()).unwrap_or(u64::MAX);
        self.field = field;
        self.population = population;
        self.quota = quota;
        self.identities = identities;
        self.ledger = EnergyLedger::new(after, self.rules.profile.residual);
        // The fresh quota's arena slots and generations start again, so an
        // installed controller backend's resident parameters, kept by slot,
        // no longer name these genomes.
        if let Some(backend) = self.controllers.backend.as_mut() {
            backend.forget();
        }
        Ok(Graft {
            center: target,
            radius,
            offset,
            cells: u64::try_from(landed.len()).unwrap_or(u64::MAX),
            displaced_field,
            displaced_organisms: measured.organisms,
            displaced_organism_energy: measured.organism_energy,
            organisms: grafted,
            lineages: u64::try_from(lineages.len()).unwrap_or(u64::MAX),
            before,
            endowment,
            after,
        })
    }
}
