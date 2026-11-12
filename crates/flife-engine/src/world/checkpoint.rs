//! World checkpoints (FL-016; plan §16.3, Appendix B.2, INV-14).
//!
//! [`World::write_checkpoint`] adds a committed world's chunks to a
//! [`CheckpointWriter`]; [`World::read_checkpoint`] rebuilds the world from
//! a verified [`CheckpointReader`]. The restored world is the same committed
//! state, bit for bit: the same digest, and every later tick publishes the
//! same report as the uninterrupted world's.
//!
//! | Chunk | Holds |
//! |---|---|
//! | configuration | the canonical configuration TOML and its hash, the slot arrangement, the numerical profile |
//! | random | the random addressing algorithm and seed; there are no stateful streams |
//! | topology | the catalog name and each member's hidden widths and parameter count |
//! | environment | the tick, the resource epoch, and each source's path ordinal and anchor |
//! | lifecycle | the organism and genome identity allocators, the event count and the genome quota |
//! | field | every plane of the committed field |
//! | genomes | every live genome: identity, topology, parent, traits and parameters |
//! | particles | every live organism: identity, genome, parent, lineage, generation, position, velocity, energy |
//! | held | per organism: held raw outputs with their evaluation tick, phase, cooldown deadline, birth tick and recent energy change |
//! | ledger | the ledger's holdings, tick count and every running sum |
//!
//! Organisms and genomes are written in identity order. Physical slots are
//! not written: no result depends on them (the C.7 arrangement test), and
//! a restored population fills its slots in identity order.
//!
//! What is derived is checked, not trusted: the configuration must
//! validate and hash to its recorded hash; the profile, seed and catalog
//! must be this build's; each phase must be the stable phase; the quota
//! rebuilt from the genomes must equal the recorded quota; and the ledger's
//! holdings must equal the restored field's and organisms' energy in every
//! bit.

use flife_core::checkpoint::{ByteReader, CheckpointReader, CheckpointWriter, Chunk, ChunkFamily};
use flife_core::clock::{Tick, stable_phase};
use flife_core::ids::{
    Generation, GenomeId, IdSequence, LineageId, OrganismId, ResourceEpoch, SourceId,
};
use flife_core::rng::FNP_PHILOX_SEMANTIC_V1;
use flife_core::{
    CompensatedSum, CoreError, EnergyLedger, Holdings, LedgerState, ResidualBudget, ValidatedConfig,
};
use flife_field::{ConeTolerance, FIELD_PLANES, FieldPair, SeededPath};
use flife_genome::{DENSE_104_36_CATALOG_V1, Genome, GenomeQuota, Traits};
use flife_neural::ACTION_WIDTH;

use super::{
    Controllers, Fields, NumericalProfile, Rules, Scratch, Sensors, Uptake, Watch, World,
    initial_sources, organism_total,
};
use crate::population::{Arrangement, Entrant, HeldOutput, Organism, Population};
use crate::reproduction::Identities;

/// Every chunk version this module writes and reads.
pub const WORLD_CHUNK_VERSION: u16 = 1;

/// The families a world's checkpoint holds, in the order they are written.
pub const WORLD_CHECKPOINT_CHUNKS: [ChunkFamily; 10] = [
    ChunkFamily::Configuration,
    ChunkFamily::Random,
    ChunkFamily::Topology,
    ChunkFamily::Environment,
    ChunkFamily::Lifecycle,
    ChunkFamily::Field,
    ChunkFamily::Genomes,
    ChunkFamily::Particles,
    ChunkFamily::Held,
    ChunkFamily::Ledger,
];

/// The longest canonical configuration a checkpoint may carry.
const CONFIGURATION_MAX_BYTES: usize = 1 << 20;

/// The longest algorithm or catalog name a checkpoint may carry.
const NAME_MAX_BYTES: usize = 256;

fn refuse(reason: impl Into<String>) -> CoreError {
    CoreError::IncompatibleCheckpoint {
        reason: reason.into(),
    }
}

/// `value` as a `u64` item count.
fn items(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// Refuse a chunk whose manifest item count is not what it holds.
fn counted(chunk: &Chunk<'_>, family: ChunkFamily, held: usize) -> Result<(), CoreError> {
    if chunk.items == items(held) {
        Ok(())
    } else {
        Err(refuse(format!(
            "the manifest counts {} items in chunk {}, which holds {held}",
            chunk.items,
            family.name()
        )))
    }
}

const fn arrangement_code(arrangement: Arrangement) -> u8 {
    match arrangement {
        Arrangement::Compact => 0,
        Arrangement::FreeList => 1,
    }
}

/// The bits of every number the profile holds.
fn profile_bits(profile: &NumericalProfile) -> [u64; 5] {
    [
        profile.residual.absolute(),
        profile.residual.per_tick(),
        profile.residual.cumulative(),
        profile.cone.absolute(),
        profile.cone.relative(),
    ]
    .map(f64::to_bits)
}

impl World {
    /// Add this committed world's chunks ([`WORLD_CHECKPOINT_CHUNKS`]) to
    /// `writer`.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] once the run stopped under
    /// `stop-and-preserve`: a stopped run is not resumable.
    /// [`CoreError::IncompatibleCheckpoint`] if `writer` already holds one
    /// of these families.
    pub fn write_checkpoint(&self, writer: &mut CheckpointWriter<'_>) -> Result<(), CoreError> {
        if let Some(fault) = &self.fault {
            return Err(CoreError::StateConflict {
                operation: "checkpoint world",
                detail: format!(
                    "the run stopped advancing tick {}: {}",
                    fault.tick.get(),
                    fault.error
                ),
            });
        }
        let version = WORLD_CHUNK_VERSION;

        writer.chunk_with(ChunkFamily::Configuration, version, 1, &[], |out| {
            out.sized(self.config.canonical_toml().as_bytes());
            out.bytes(self.config.config_hash().as_bytes());
            out.u8(arrangement_code(self.population.arrangement()));
            for bits in profile_bits(&self.rules.profile) {
                out.u64(bits);
            }
            Ok(())
        })?;

        writer.chunk_with(
            ChunkFamily::Random,
            version,
            1,
            &[ChunkFamily::Configuration],
            |out| {
                out.sized(FNP_PHILOX_SEMANTIC_V1.as_bytes());
                out.u64(self.rules.rng.seed());
                Ok(())
            },
        )?;

        let catalog = &self.rules.catalog;
        writer.chunk_with(
            ChunkFamily::Topology,
            version,
            items(catalog.len()),
            &[ChunkFamily::Configuration],
            |out| {
                out.sized(DENSE_104_36_CATALOG_V1.as_bytes());
                out.u64(items(catalog.len()));
                for id in catalog.ids() {
                    let (widths, parameters) =
                        catalog.topology(id).map_or((&[][..], 0), |topology| {
                            (topology.hidden_widths(), topology.parameter_count())
                        });
                    out.u64(items(widths.len()));
                    for width in widths {
                        out.u32(*width);
                    }
                    out.u64(items(parameters));
                }
                Ok(())
            },
        )?;

        writer.chunk_with(
            ChunkFamily::Environment,
            version,
            items(self.paths.len()),
            &[ChunkFamily::Configuration],
            |out| {
                out.u64(self.tick.get());
                out.u64(self.epoch.get());
                out.u64(items(self.paths.len()));
                for path in &self.paths {
                    out.u32(path.source().get());
                    out.u64(path.ordinal());
                    for value in path.segment().anchor() {
                        out.f64(value);
                    }
                }
                Ok(())
            },
        )?;

        writer.chunk_with(
            ChunkFamily::Lifecycle,
            version,
            1,
            &[ChunkFamily::Configuration],
            |out| {
                out.option_u64(self.identities.organisms.next_raw());
                out.option_u64(self.identities.genomes.next_raw());
                out.u64(self.events);
                out.u64(self.quota.logical_bytes());
                out.u64(self.quota.allocated_bytes());
                out.u64(self.quota.live());
                Ok(())
            },
        )?;

        let field = self.field.current();
        let grid = field.grid();
        let values: usize = (0..FIELD_PLANES)
            .map(|plane| field.plane(plane).map_or(0, <[f32]>::len))
            .sum();
        writer.chunk_with(
            ChunkFamily::Field,
            version,
            items(values),
            &[ChunkFamily::Configuration],
            |out| {
                out.u32(grid.cells_x());
                out.u32(grid.cells_y());
                out.u32(FIELD_PLANES);
                for plane in 0..FIELD_PLANES {
                    for value in field.plane(plane).unwrap_or(&[]) {
                        out.f32(*value);
                    }
                }
                Ok(())
            },
        )?;

        let live = self.population.len();
        writer.chunk_with(
            ChunkFamily::Genomes,
            version,
            items(live),
            &[ChunkFamily::Topology],
            |out| {
                out.u64(items(live));
                for organism in self.population.iter() {
                    let genome = self.population.genome(organism.id).ok_or_else(|| {
                        CoreError::StateConflict {
                            operation: "checkpoint world",
                            detail: format!("{} has no genome", organism.id),
                        }
                    })?;
                    out.u64(genome.id().get());
                    out.u64(items(genome.topology().index()));
                    out.option_u64(genome.parent().map(GenomeId::get));
                    let traits = genome.traits();
                    for value in traits.affinity().iter().chain(traits.motion_response()) {
                        out.f64(*value);
                    }
                    out.f64(traits.sensing_scale());
                    out.u64(items(genome.parameters().len()));
                    for value in genome.parameters() {
                        out.f32(*value);
                    }
                }
                Ok(())
            },
        )?;

        writer.chunk_with(
            ChunkFamily::Particles,
            version,
            items(live),
            &[ChunkFamily::Genomes],
            |out| {
                out.u64(items(live));
                for organism in self.population.iter() {
                    out.u64(organism.id.get());
                    out.u64(organism.genome.get());
                    out.option_u64(organism.parent.map(OrganismId::get));
                    out.u64(organism.lineage.get());
                    out.u32(organism.generation.get());
                    for value in organism
                        .position
                        .into_iter()
                        .chain(organism.velocity)
                        .chain([organism.energy])
                    {
                        out.f64(value);
                    }
                }
                Ok(())
            },
        )?;

        writer.chunk_with(
            ChunkFamily::Held,
            version,
            items(live),
            &[ChunkFamily::Particles],
            |out| {
                out.u64(items(live));
                for organism in self.population.iter() {
                    out.u64(organism.id.get());
                    out.option_u64(organism.held.evaluated_at().map(Tick::get));
                    if let Some(outputs) = organism.held.outputs() {
                        for value in outputs {
                            out.f32(*value);
                        }
                    }
                    out.u32(organism.phase);
                    out.u64(organism.cooldown_deadline.get());
                    out.u64(organism.birth_tick.get());
                    out.f64(organism.energy_change_rate);
                }
                Ok(())
            },
        )?;

        let state = self.ledger.state();
        writer.chunk_with(
            ChunkFamily::Ledger,
            version,
            1,
            &[ChunkFamily::Field, ChunkFamily::Particles],
            |out| {
                for holdings in [state.initial, state.last] {
                    out.f64(holdings.field());
                    out.f64(holdings.organisms());
                }
                out.option_u64(state.next_tick.map(Tick::get));
                out.u64(state.ticks);
                for sum in [
                    state.injected,
                    state.removed,
                    state.dissipated,
                    state.turnover,
                    state.residual,
                ] {
                    for value in sum.parts() {
                        out.f64(value);
                    }
                }
                out.f64(state.largest_tick_residual);
                for value in state.drift {
                    out.f64(value);
                }
                Ok(())
            },
        )
    }

    /// The world `reader` holds, checked against `profile` (each chunk's
    /// checks are listed in this module's source documentation). It has the
    /// configuration's worker count and no controller backend.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] for a chunk this build does not
    /// read, any inconsistency between the chunks, a configuration, seed,
    /// catalog or profile other than this build's, or a value outside its
    /// domain; and the errors of founding the world's operators.
    pub fn read_checkpoint(
        reader: &CheckpointReader<'_>,
        profile: NumericalProfile,
    ) -> Result<Self, CoreError> {
        let version = WORLD_CHUNK_VERSION;

        // Configuration, then the rules it implies.
        let chunk = reader.versioned(ChunkFamily::Configuration, version)?;
        let mut input = ByteReader::new(chunk.bytes, "configuration chunk");
        let text = std::str::from_utf8(input.sized(CONFIGURATION_MAX_BYTES)?)
            .map_err(|_| refuse("the configuration is not UTF-8"))?;
        let recorded = input.array::<32>()?;
        let arrangement = match input.u8()? {
            0 => Arrangement::Compact,
            1 => Arrangement::FreeList,
            other => return Err(refuse(format!("unknown slot arrangement {other}"))),
        };
        let mut bits = [0_u64; 5];
        for slot in &mut bits {
            *slot = input.u64()?;
        }
        input.finish()?;
        counted(&chunk, ChunkFamily::Configuration, 1)?;
        let config = ValidatedConfig::from_toml(text)
            .map_err(|error| refuse(format!("the configuration does not validate: {error}")))?;
        if config.config_hash().as_bytes() != &recorded {
            return Err(refuse(
                "the configuration does not hash to its recorded hash",
            ));
        }
        if reader.model() != config.config().model {
            return Err(refuse(format!(
                "the checkpoint names model {}, its configuration {}",
                reader.model(),
                config.config().model
            )));
        }
        if bits != profile_bits(&profile) {
            let [absolute, per_tick, cumulative, cone_absolute, cone_relative] =
                bits.map(f64::from_bits);
            let recorded = ResidualBudget::new(absolute, per_tick, cumulative)
                .ok()
                .zip(ConeTolerance::new(cone_absolute, cone_relative).ok());
            return Err(refuse(format!(
                "the checkpoint was written under numerical profile {recorded:?}, not {profile:?}"
            )));
        }
        let rules = Rules::from_config(&config, profile)?;

        let chunk = reader.versioned(ChunkFamily::Random, version)?;
        let mut input = ByteReader::new(chunk.bytes, "random chunk");
        let algorithm = input.sized(NAME_MAX_BYTES)?;
        let seed = input.u64()?;
        input.finish()?;
        counted(&chunk, ChunkFamily::Random, 1)?;
        if algorithm != FNP_PHILOX_SEMANTIC_V1.as_bytes() || seed != rules.rng.seed() {
            return Err(refuse(format!(
                "random addressing {} with seed {seed}; this world uses {FNP_PHILOX_SEMANTIC_V1} with seed {}",
                String::from_utf8_lossy(algorithm),
                rules.rng.seed()
            )));
        }

        let chunk = reader.versioned(ChunkFamily::Topology, version)?;
        let mut input = ByteReader::new(chunk.bytes, "topology chunk");
        let name = input.sized(NAME_MAX_BYTES)?;
        let catalog = &rules.catalog;
        let mut matches = name == DENSE_104_36_CATALOG_V1.as_bytes()
            && input.count(catalog.len(), 1)? == catalog.len();
        for id in catalog.ids() {
            let (widths, parameters) = catalog.topology(id).map_or((&[][..], 0), |topology| {
                (topology.hidden_widths(), topology.parameter_count())
            });
            let count = input.count(NAME_MAX_BYTES, 4)?;
            let mut recorded = Vec::with_capacity(count);
            for _ in 0..count {
                recorded.push(input.u32()?);
            }
            matches &= recorded == widths && input.u64()? == items(parameters);
        }
        input.finish()?;
        counted(&chunk, ChunkFamily::Topology, catalog.len())?;
        if !matches {
            return Err(refuse("the topology catalog is not this build's"));
        }

        // Environment and lifecycle.
        let model = config.config();
        let chunk = reader.versioned(ChunkFamily::Environment, version)?;
        let mut input = ByteReader::new(chunk.bytes, "environment chunk");
        let tick = Tick::new(input.u64()?);
        let epoch = ResourceEpoch::new(input.u64()?);
        let sources = usize::try_from(model.sources.count).unwrap_or(usize::MAX);
        let count = input.count(sources, 28)?;
        if count != sources {
            return Err(refuse(format!(
                "{count} source paths for {sources} sources"
            )));
        }
        let mut paths = Vec::with_capacity(count);
        for index in 0..model.sources.count {
            let source = input.u32()?;
            if source != index {
                return Err(refuse(format!("source path {source} where {index} is due")));
            }
            let ordinal = input.u64()?;
            let anchor = [input.f64()?, input.f64()?];
            paths.push(
                SeededPath::resume(
                    &rules.rng,
                    &rules.grid,
                    SourceId::new(source),
                    ordinal,
                    anchor,
                    model.sources.path_speed,
                    config.report().source_segment_ticks,
                    rules.step,
                )
                .map_err(|error| refuse(format!("source {source}'s path: {error}")))?,
            );
        }
        input.finish()?;
        counted(&chunk, ChunkFamily::Environment, paths.len())?;

        let chunk = reader.versioned(ChunkFamily::Lifecycle, version)?;
        let mut input = ByteReader::new(chunk.bytes, "lifecycle chunk");
        let identities = Identities {
            organisms: IdSequence::resume(input.option_u64()?),
            genomes: IdSequence::resume(input.option_u64()?),
        };
        let events = input.u64()?;
        let quota_recorded = [input.u64()?, input.u64()?, input.u64()?];
        input.finish()?;
        counted(&chunk, ChunkFamily::Lifecycle, 1)?;

        // The field.
        let mut field = FieldPair::zeroed(rules.grid, &flife_field::BufferLimits::host())?;
        let chunk = reader.versioned(ChunkFamily::Field, version)?;
        let mut input = ByteReader::new(chunk.bytes, "field chunk");
        let shape = [input.u32()?, input.u32()?, input.u32()?];
        if shape != [rules.grid.cells_x(), rules.grid.cells_y(), FIELD_PLANES] {
            return Err(refuse(format!(
                "a {shape:?} field for a {} × {} grid of {FIELD_PLANES} planes",
                rules.grid.cells_x(),
                rules.grid.cells_y()
            )));
        }
        let state = field.current_mut();
        for plane in 0..FIELD_PLANES {
            let values = state
                .plane_mut(plane)
                .ok_or_else(|| CoreError::LayoutMismatch {
                    buffer: "checkpoint field",
                    detail: format!("plane {plane} is missing"),
                })?;
            for value in values {
                let read = input.f32()?;
                if !read.is_finite() {
                    return Err(refuse(format!("field plane {plane} holds {read}")));
                }
                *value = read;
            }
        }
        input.finish()?;
        counted(&chunk, ChunkFamily::Field, field.current().as_slice().len())?;

        // Genomes, rebuilt through the run's own quota and seal.
        let maximum = usize::try_from(rules.maximum_population).unwrap_or(usize::MAX);
        let parameters_max = usize::try_from(catalog.maximum_parameters()).unwrap_or(usize::MAX);
        let mut quota = GenomeQuota::from_config(&config)?;
        let chunk = reader.versioned(ChunkFamily::Genomes, version)?;
        let mut input = ByteReader::new(chunk.bytes, "genomes chunk");
        let live = input.count(maximum, 8)?;
        let mut genomes = Vec::with_capacity(live);
        for _ in 0..live {
            let id = GenomeId::new(input.u64()?);
            let index = input.u64()?;
            let topology = usize::try_from(index)
                .ok()
                .and_then(|index| catalog.ids().nth(index))
                .ok_or_else(|| {
                    refuse(format!("{id} has topology {index}, not a catalog member"))
                })?;
            let parent = input.option_u64()?.map(GenomeId::new);
            let mut affinity = [0.0; flife_genome::AFFINITY_TERMS];
            for value in &mut affinity {
                *value = input.f64()?;
            }
            let mut motion = [0.0; flife_genome::MOTION_RESPONSE_TERMS];
            for value in &mut motion {
                *value = input.f64()?;
            }
            let sensing = input.f64()?;
            let traits = Traits::new(affinity, motion, sensing, &rules.limits)
                .map_err(|error| refuse(format!("{id}'s traits: {error}")))?;
            let count = input.count(parameters_max, 4)?;
            let mut buffer = quota.allocate(catalog, topology)?;
            if buffer.values().len() != count {
                quota.abandon(buffer)?;
                return Err(refuse(format!(
                    "{id} has {count} parameters; its topology has a different count"
                )));
            }
            for value in buffer.values_mut() {
                *value = input.f32()?;
            }
            let genome = Genome::seal(buffer, id, catalog, traits, parent, &rules.limits).or_else(
                |unsealed| {
                    quota.abandon(unsealed.buffer)?;
                    Err(refuse(format!("{id}'s parameters: {}", unsealed.error)))
                },
            )?;
            genomes.push(genome);
        }
        input.finish()?;
        counted(&chunk, ChunkFamily::Genomes, live)?;
        if [quota.logical_bytes(), quota.allocated_bytes(), quota.live()] != quota_recorded {
            return Err(refuse(format!(
                "the genomes' quota is {:?}; the checkpoint records {quota_recorded:?}",
                [quota.logical_bytes(), quota.allocated_bytes(), quota.live()]
            )));
        }

        // Organisms: particles and held state, row by row.
        let chunk = reader.versioned(ChunkFamily::Particles, version)?;
        counted(&chunk, ChunkFamily::Particles, live)?;
        let mut particles = ByteReader::new(chunk.bytes, "particles chunk");
        let chunk = reader.versioned(ChunkFamily::Held, version)?;
        counted(&chunk, ChunkFamily::Held, live)?;
        let mut held = ByteReader::new(chunk.bytes, "held chunk");
        if particles.count(maximum, 8)? != live || held.count(maximum, 8)? != live {
            return Err(refuse("the particle, held and genome counts differ"));
        }
        let mut entrants = Vec::with_capacity(live);
        for genome in genomes {
            let id = OrganismId::new(particles.u64()?);
            let genome_id = GenomeId::new(particles.u64()?);
            if genome_id != genome.id() {
                return Err(refuse(format!(
                    "{id} has {genome_id}, but the genome in its place is {}",
                    genome.id()
                )));
            }
            let parent = particles.option_u64()?.map(OrganismId::new);
            let lineage = LineageId::new(particles.u64()?);
            let generation = Generation::new(particles.u32()?);
            let position = [particles.f64()?, particles.f64()?];
            let velocity = [particles.f64()?, particles.f64()?];
            let energy = particles.f64()?;

            let held_id = OrganismId::new(held.u64()?);
            if held_id != id {
                return Err(refuse(format!(
                    "held state for {held_id} where {id}'s is due"
                )));
            }
            let held_output = match held.option_u64()? {
                Some(at) => {
                    let mut outputs = [0.0; ACTION_WIDTH];
                    for value in &mut outputs {
                        *value = held.f32()?;
                    }
                    HeldOutput::evaluated(Tick::new(at), outputs)
                        .map_err(|error| refuse(format!("{id}'s held output: {error}")))?
                }
                None => HeldOutput::INVALID,
            };
            let phase = held.u32()?;
            if phase != stable_phase(id, rules.interval)? {
                return Err(refuse(format!(
                    "{id} has phase {phase}, not its stable phase"
                )));
            }
            let cooldown_deadline = Tick::new(held.u64()?);
            let birth_tick = Tick::new(held.u64()?);
            let energy_change_rate = held.f64()?;
            if birth_tick > tick || !energy_change_rate.is_finite() {
                return Err(refuse(format!(
                    "{id} is born at tick {} with energy change {energy_change_rate} in a world at tick {}",
                    birth_tick.get(),
                    tick.get()
                )));
            }
            entrants.push(Entrant {
                organism: Organism {
                    id,
                    genome: genome_id,
                    parent,
                    lineage,
                    generation,
                    birth_tick,
                    position: rules.grid.wrap_position(position).and_then(|wrapped| {
                        if wrapped.map(f64::to_bits) == position.map(f64::to_bits) {
                            Ok(position)
                        } else {
                            Err(refuse(format!(
                                "{id}'s position {position:?} is not wrapped"
                            )))
                        }
                    })?,
                    velocity,
                    energy,
                    energy_change_rate,
                    held: held_output,
                    phase,
                    cooldown_deadline,
                },
                genome,
            });
        }
        particles.finish()?;
        held.finish()?;
        if let Some(last) = entrants.last() {
            let past = |next: Option<u64>, id: u64| next.is_some_and(|next| next > id);
            if !past(identities.organisms.next_raw(), last.organism.id.get())
                || !entrants
                    .iter()
                    .all(|entrant| past(identities.genomes.next_raw(), entrant.genome.id().get()))
            {
                return Err(refuse(
                    "an identity allocator would reissue a live identity",
                ));
            }
        }
        let mut population = Population::from_config(arrangement, &config)?;
        population
            .admit(&mut entrants)
            .map_err(|error| refuse(format!("the organisms are not admissible: {error}")))?;

        // The ledger, which must close at exactly the restored energy.
        let chunk = reader.versioned(ChunkFamily::Ledger, version)?;
        let mut input = ByteReader::new(chunk.bytes, "ledger chunk");
        let initial = Holdings::new(input.f64()?, input.f64()?)
            .map_err(|error| refuse(format!("ledger initial holdings: {error}")))?;
        let last = Holdings::new(input.f64()?, input.f64()?)
            .map_err(|error| refuse(format!("ledger holdings: {error}")))?;
        let next_tick = input.option_u64()?.map(Tick::new);
        let ticks = input.u64()?;
        let mut sums = [CompensatedSum::ZERO; 5];
        for sum in &mut sums {
            *sum = CompensatedSum::from_parts([input.f64()?, input.f64()?]);
        }
        let largest_tick_residual = input.f64()?;
        let mut drift = [0.0; 5];
        for value in &mut drift {
            *value = input.f64()?;
        }
        input.finish()?;
        counted(&chunk, ChunkFamily::Ledger, 1)?;
        let [injected, removed, dissipated, turnover, residual] = sums;
        let ledger = EnergyLedger::restore(
            LedgerState {
                initial,
                last,
                next_tick,
                ticks,
                injected,
                removed,
                dissipated,
                turnover,
                residual,
                largest_tick_residual,
                drift,
            },
            profile.residual,
        )?;
        let measured = Holdings::new(field.current().energy_total(), organism_total(&population))?;
        if [measured.field(), measured.organisms()].map(f64::to_bits)
            != [last.field(), last.organisms()].map(f64::to_bits)
        {
            return Err(refuse(format!(
                "the ledger closed at {last:?}, the restored world holds {measured:?}"
            )));
        }
        if next_tick.is_some_and(|next| next != tick) {
            return Err(refuse(format!(
                "the ledger's next tick is {next_tick:?}, the world is at tick {}",
                tick.get()
            )));
        }

        let sources = initial_sources(&config, &rules)?;
        let scratch = Scratch::new(&config, &rules, sources.len())?;
        Ok(Self {
            config,
            rules,
            tick,
            epoch,
            events,
            field,
            population,
            sources,
            paths,
            identities,
            quota,
            ledger,
            fault: None,
            scratch,
            controllers: Controllers::default(),
            fields: Fields::default(),
            sensors: Sensors::default(),
            uptake: Uptake::default(),
            // A checkpoint holds no watch: a restored world watches nothing.
            watch: Watch::default(),
            // Nor any ancestry: a restored world traces none until told to.
            ancestry: None,
        })
    }
}
