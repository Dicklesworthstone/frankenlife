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

use flife_core::binary16::F16;
use flife_core::checkpoint::{ByteReader, CheckpointReader, CheckpointWriter, ChunkFamily};
use flife_core::clock::{Tick, stable_phase};
use flife_core::ids::{
    Generation, GenomeId, IdSequence, LineageId, OrganismId, ResourceEpoch, SourceId,
};
use flife_core::model::is_v2_model;
use flife_core::rng::FNP_PHILOX_SEMANTIC_V1;
use flife_core::{
    CompensatedSum, CoreError, EnergyLedger, Holdings, LedgerState, ResidualBudget, ValidatedConfig,
};
use flife_field::{ConeTolerance, FieldPair, SeededPath};
use flife_genome::{Genome, GenomeQuota, Traits, WeightsMut};
use flife_neural::{ACTION_WIDTH, MAX_ACTION_WIDTH, MAXIMUM_RECURRENT_STATE_WIDTH, Weights};

use super::{
    Controllers, Fields, NumericalProfile, Rules, Scratch, Sensors, Settle, Uptake, Watch, World,
    initial_source_kernels, initial_sources, organism_total,
};
use crate::emission::INITIAL_SECRETORY_READINESS;
use crate::population::{Arrangement, Entrant, HeldOutput, Organism, Population};
use crate::reproduction::Identities;

/// Every baseline chunk version this module writes and reads.
pub const WORLD_CHUNK_VERSION: u16 = 1;

/// `Held` chunk version carrying per-organism recurrent internal state
/// (`independent-recurrent-mlp-v1`, FL-054, plan §25.2).
pub const WORLD_RECURRENT_HELD_VERSION: u16 = 2;

/// Modular `field-ecology-v2*` chunk version carrying the feature bitmask,
/// dynamic controller ABI dimensions, variable-width held outputs, and
/// newborn epigenetic state (V2-01).
pub const WORLD_V2_CHUNK_VERSION: u16 = 3;

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

/// Refuse a chunk whose manifest item count, `manifest`, is not what it
/// holds.
fn counted(manifest: u64, family: ChunkFamily, held: usize) -> Result<(), CoreError> {
    if manifest == items(held) {
        Ok(())
    } else {
        Err(refuse(format!(
            "the manifest counts {manifest} items in chunk {}, which holds {held}",
            family.name()
        )))
    }
}

/// The most a genome record's fixed part takes before its parameter count:
/// its identity, topology index, parent (a flag and an identity), traits
/// and sensing. A stream's window is filled to this before each record.
/// Parameterized by `rules` so v2 trait extensions (polarity, cadherin, etc.)
/// grow the refill window dynamically without under-filling `ByteReader`.
fn genome_header_bytes(_rules: &Rules) -> usize {
    8 + 8 + 9 + 8 * (flife_genome::AFFINITY_TERMS + flife_genome::MOTION_RESPONSE_TERMS) + 8
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
        let is_v2 = self.rules.variant.is_v2();
        let version = if is_v2 {
            WORLD_V2_CHUNK_VERSION
        } else {
            WORLD_CHUNK_VERSION
        };

        writer.chunk_with(ChunkFamily::Configuration, version, 1, &[], |out| {
            out.sized(self.config.canonical_toml().as_bytes());
            out.bytes(self.config.config_hash().as_bytes());
            out.u8(arrangement_code(self.population.arrangement()));
            for bits in profile_bits(&self.rules.profile) {
                out.u64(bits);
            }
            if is_v2 {
                out.u64(self.rules.v2.feature_bitmask());
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
                out.sized(catalog.name().as_bytes());
                out.u64(items(catalog.len()));
                if is_v2 {
                    let abi = self.rules.v2.abi();
                    out.u32(abi.input_width);
                    out.u32(abi.action_width);
                    out.u32(abi.raw_output_width);
                    out.u32(abi.recurrent_state_width);
                }
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
                if is_v2 {
                    out.u64(items(self.source_kernels.len()));
                    for kernel in &self.source_kernels {
                        out.f64(kernel.sigma());
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
        let planes = field.planes();
        let values: usize = (0..planes)
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
                out.u32(planes);
                for plane in 0..planes {
                    for value in field.plane(plane).unwrap_or(&[]) {
                        out.f32(*value);
                    }
                }
                if is_v2 {
                    if let Some((gx, gy)) = field.vascular_edges() {
                        out.u8(1);
                        for &value in gx {
                            out.f32(value);
                        }
                        for &value in gy {
                            out.f32(value);
                        }
                    } else {
                        out.u8(0);
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
                    // Each value in the run's weight dtype: an F32's bits,
                    // or a binary16's.
                    match genome.parameters() {
                        Weights::F32(values) => values.iter().for_each(|value| out.f32(*value)),
                        Weights::F16(values) => {
                            values.iter().for_each(|value| out.u16(value.to_bits()));
                        }
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

        let state_width = self.rules.recurrent_state_width;
        let held_version = if is_v2 {
            WORLD_V2_CHUNK_VERSION
        } else if state_width > 0 {
            WORLD_RECURRENT_HELD_VERSION
        } else {
            version
        };
        writer.chunk_with(
            ChunkFamily::Held,
            held_version,
            items(live),
            &[ChunkFamily::Particles],
            |out| {
                out.u64(items(live));
                for organism in self.population.iter() {
                    out.u64(organism.id.get());
                    out.option_u64(organism.held.evaluated_at().map(Tick::get));
                    if let Some(outputs) = organism.held.outputs() {
                        if is_v2 {
                            out.u8(u8::try_from(outputs.len()).unwrap_or(0));
                        }
                        for value in outputs {
                            out.f32(*value);
                        }
                    }
                    out.u32(organism.phase);
                    out.u64(organism.cooldown_deadline.get());
                    out.u64(organism.birth_tick.get());
                    out.f64(organism.energy_change_rate);
                    if is_v2 || state_width > 0 {
                        let state = organism.held.recurrent_state();
                        out.u8(u8::try_from(state.len()).unwrap_or(0));
                        for value in state {
                            out.f32(*value);
                        }
                    }
                    if is_v2 {
                        out.f32(organism.secretory_readiness);
                        out.u64(organism.emission_cooldown_deadline.get());
                    }
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
        // Configuration, then the rules it implies.
        let chunk = reader.chunk(ChunkFamily::Configuration)?;
        let version = match chunk.version {
            WORLD_CHUNK_VERSION | WORLD_V2_CHUNK_VERSION => chunk.version,
            other => {
                return Err(refuse(format!(
                    "chunk configuration is version {other}, this build reads {WORLD_CHUNK_VERSION} \
                     or {WORLD_V2_CHUNK_VERSION}"
                )));
            }
        };
        let is_v2_chunk = version == WORLD_V2_CHUNK_VERSION;
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
        let recorded_bitmask = if is_v2_chunk {
            Some(input.u64()?)
        } else {
            None
        };
        input.finish()?;
        counted(chunk.items, ChunkFamily::Configuration, 1)?;
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
        let model_is_v2 = is_v2_model(&config.config().model);
        if model_is_v2 != is_v2_chunk {
            return Err(refuse(format!(
                "model {} is incompatible with checkpoint chunk version {version}",
                config.config().model
            )));
        }
        if let Some(bitmask) = recorded_bitmask
            && bitmask != config.v2_feature_bitmask()
        {
            return Err(refuse(format!(
                "checkpoint v2 feature bitmask {bitmask:#x} does not match configuration \
                 bitmask {:#x}",
                config.v2_feature_bitmask()
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
        counted(chunk.items, ChunkFamily::Random, 1)?;
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
        let mut matches =
            name == catalog.name().as_bytes() && input.count(catalog.len(), 1)? == catalog.len();
        if is_v2_chunk {
            let abi = rules.v2.abi();
            let recorded_abi = [input.u32()?, input.u32()?, input.u32()?, input.u32()?];
            matches &= recorded_abi
                == [
                    abi.input_width,
                    abi.action_width,
                    abi.raw_output_width,
                    abi.recurrent_state_width,
                ];
        }
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
        counted(chunk.items, ChunkFamily::Topology, catalog.len())?;
        if !matches {
            return Err(refuse("the topology catalog is not this build's"));
        }

        // Environment and lifecycle.
        let model = config.config();
        let initial_source_list = initial_sources(&config, &rules)?;
        let source_kernels = initial_source_kernels(&rules, &initial_source_list)?;
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
        if is_v2_chunk {
            let kernel_count = input.count(sources, 8)?;
            if kernel_count != source_kernels.len() {
                return Err(refuse(format!(
                    "{kernel_count} source sigmas where {} are due",
                    source_kernels.len()
                )));
            }
            for (index, expected) in source_kernels.iter().enumerate() {
                let recorded_sigma = input.f64()?;
                if recorded_sigma.to_bits() != expected.sigma().to_bits() {
                    return Err(refuse(format!(
                        "source {index} sigma {recorded_sigma:?} does not match seeded {:?}",
                        expected.sigma()
                    )));
                }
            }
        }
        input.finish()?;
        counted(chunk.items, ChunkFamily::Environment, paths.len())?;

        let chunk = reader.versioned(ChunkFamily::Lifecycle, version)?;
        let mut input = ByteReader::new(chunk.bytes, "lifecycle chunk");
        let identities = Identities {
            organisms: IdSequence::resume(input.option_u64()?),
            genomes: IdSequence::resume(input.option_u64()?),
        };
        let events = input.u64()?;
        let quota_recorded = [input.u64()?, input.u64()?, input.u64()?];
        input.finish()?;
        counted(chunk.items, ChunkFamily::Lifecycle, 1)?;

        // The field.
        let limits = flife_field::BufferLimits::host();
        let expected_planes = rules.v2.field_planes();
        let mut field = FieldPair::zeroed_with_planes(rules.grid, expected_planes, &limits)?;
        let chunk = reader.versioned(ChunkFamily::Field, version)?;
        let mut input = ByteReader::new(chunk.bytes, "field chunk");
        let shape = [input.u32()?, input.u32()?, input.u32()?];
        if shape != [rules.grid.cells_x(), rules.grid.cells_y(), expected_planes] {
            return Err(refuse(format!(
                "a {shape:?} field for a {} × {} grid of {expected_planes} planes",
                rules.grid.cells_x(),
                rules.grid.cells_y()
            )));
        }
        let state = field.current_mut();
        for plane in 0..expected_planes {
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
        if is_v2_chunk {
            let has_vascular = input.u8()?;
            let expected_vascular = u8::from(rules.v2.vascular().is_some());
            if has_vascular != expected_vascular {
                return Err(refuse(format!(
                    "field chunk vascular flag {has_vascular} does not match configuration {expected_vascular}"
                )));
            }
            if let Some(vascular) = rules.v2.vascular() {
                let cells =
                    usize::try_from(rules.grid.cell_count()).map_err(|_| CoreError::Overflow {
                        quantity: "grid cells",
                    })?;
                field.enable_vascular(vascular.initial_conductance(), &limits)?;
                let mut gx = Vec::with_capacity(cells);
                let mut gy = Vec::with_capacity(cells);
                for _ in 0..cells {
                    gx.push(input.f32()?);
                }
                for _ in 0..cells {
                    gy.push(input.f32()?);
                }
                field
                    .current_mut()
                    .set_vascular_edges(&gx, &gy)
                    .map_err(|error| refuse(format!("vascular edges: {error}")))?;
            }
        }
        input.finish()?;
        counted(
            chunk.items,
            ChunkFamily::Field,
            field.current().as_slice().len(),
        )?;

        // Genomes, rebuilt through the run's own quota and seal, a record
        // at a time from a window over the chunk: a streamed chunk is never
        // held whole (FL-043), and is verified when the stream finishes.
        let maximum = usize::try_from(rules.maximum_population).unwrap_or(usize::MAX);
        let parameters_max = usize::try_from(catalog.maximum_parameters()).unwrap_or(usize::MAX);
        let mut quota = GenomeQuota::from_config(&config)?;
        let width = usize::try_from(quota.dtype().bytes()).unwrap_or(usize::MAX);
        let mut stream = reader.stream(ChunkFamily::Genomes, version)?;
        let live = stream.count(maximum, 8)?;
        let header_bytes = genome_header_bytes(&rules);
        let mut genomes = Vec::with_capacity(live);
        for _ in 0..live {
            stream.fill(header_bytes)?;
            let mut input = ByteReader::new(stream.window(), "genomes chunk");
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
            let used = stream.window().len() - input.remaining();
            stream.advance(used);
            let traits = Traits::new(affinity, motion, sensing, &rules.limits)
                .map_err(|error| refuse(format!("{id}'s traits: {error}")))?;
            let count = stream.count(parameters_max, width)?;
            let mut buffer = quota.allocate(catalog, topology)?;
            if buffer.values().len() != count {
                quota.abandon(buffer)?;
                return Err(refuse(format!(
                    "{id} has {count} parameters; its topology has a different count"
                )));
            }
            if let Err(error) = stream.fill(count.saturating_mul(width)) {
                quota.abandon(buffer)?;
                return Err(error);
            }
            let mut input = ByteReader::new(stream.window(), "genomes chunk");
            let read = match buffer.values_mut() {
                WeightsMut::F32(values) => values
                    .iter_mut()
                    .try_for_each(|value| input.f32().map(|read| *value = read)),
                WeightsMut::F16(values) => values
                    .iter_mut()
                    .try_for_each(|value| input.u16().map(|read| *value = F16::from_bits(read))),
            };
            let used = stream.window().len() - input.remaining();
            stream.advance(used);
            if let Err(error) = read {
                quota.abandon(buffer)?;
                return Err(error);
            }
            let genome = Genome::seal(buffer, id, catalog, traits, parent, &rules.limits).or_else(
                |unsealed| {
                    quota.abandon(unsealed.buffer)?;
                    Err(refuse(format!("{id}'s parameters: {}", unsealed.error)))
                },
            )?;
            genomes.push(genome);
        }
        let (window, genome_items) = (stream.held_bytes(), stream.items());
        stream.finish()?;
        counted(genome_items, ChunkFamily::Genomes, live)?;
        if [quota.logical_bytes(), quota.allocated_bytes(), quota.live()] != quota_recorded {
            return Err(refuse(format!(
                "the genomes' quota is {:?}; the checkpoint records {quota_recorded:?}",
                [quota.logical_bytes(), quota.allocated_bytes(), quota.live()]
            )));
        }

        // Organisms: particles and held state, row by row.
        let chunk = reader.versioned(ChunkFamily::Particles, version)?;
        counted(chunk.items, ChunkFamily::Particles, live)?;
        let mut particles = ByteReader::new(chunk.bytes, "particles chunk");
        let state_width = rules.recurrent_state_width;
        let expected_action_width = rules.v2.actions().action_width();
        let held_version = if is_v2_chunk {
            WORLD_V2_CHUNK_VERSION
        } else if state_width > 0 {
            WORLD_RECURRENT_HELD_VERSION
        } else {
            version
        };
        let chunk = reader.versioned(ChunkFamily::Held, held_version)?;
        counted(chunk.items, ChunkFamily::Held, live)?;
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
            let evaluated_raw = match held.option_u64()? {
                Some(at) => {
                    let out_len = if is_v2_chunk {
                        let len = usize::from(held.u8()?);
                        if len != expected_action_width || len > MAX_ACTION_WIDTH {
                            return Err(refuse(format!(
                                "{id} holds {len} outputs, expected {expected_action_width}"
                            )));
                        }
                        len
                    } else {
                        ACTION_WIDTH
                    };
                    let mut outputs = [0.0; MAX_ACTION_WIDTH];
                    for value in &mut outputs[..out_len] {
                        *value = held.f32()?;
                    }
                    Some((Tick::new(at), outputs, out_len))
                }
                None => None,
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
            let held_output = if is_v2_chunk {
                let len = usize::from(held.u8()?);
                let mut state = [0.0_f32; MAXIMUM_RECURRENT_STATE_WIDTH];
                if len > MAXIMUM_RECURRENT_STATE_WIDTH {
                    return Err(refuse(format!(
                        "{id} has recurrent state width {len}, above {MAXIMUM_RECURRENT_STATE_WIDTH}"
                    )));
                }
                for value in &mut state[..len] {
                    *value = held.f32()?;
                }
                if state_width == 0 {
                    if len != 0 {
                        return Err(refuse(format!(
                            "{id} carries {len} recurrent state values in a stateless v2 world"
                        )));
                    }
                    match evaluated_raw {
                        Some((at, outputs, out_len)) => {
                            HeldOutput::evaluated_slice(at, &outputs[..out_len])
                                .map_err(|error| refuse(format!("{id}'s held output: {error}")))?
                        }
                        None => HeldOutput::INVALID,
                    }
                } else {
                    match evaluated_raw {
                        Some((at, outputs, out_len)) => {
                            if len != state_width {
                                return Err(refuse(format!(
                                    "{id} has recurrent state width {len}, expected {state_width}"
                                )));
                            }
                            HeldOutput::evaluated_recurrent_slice(
                                at,
                                &outputs[..out_len],
                                &state[..len],
                            )
                            .map_err(|error| {
                                refuse(format!("{id}'s held recurrent state: {error}"))
                            })?
                        }
                        None => {
                            if len == 0 {
                                HeldOutput::INVALID
                            } else if len == state_width {
                                HeldOutput::newborn_with_state(&state[..len]).map_err(|error| {
                                    refuse(format!("{id}'s newborn recurrent state: {error}"))
                                })?
                            } else {
                                return Err(refuse(format!(
                                    "{id} holds no output but carries {len} recurrent state values \
                                     (expected 0 or {state_width})"
                                )));
                            }
                        }
                    }
                }
            } else if state_width > 0 {
                let len = usize::from(held.u8()?);
                let mut state = [0.0_f32; MAXIMUM_RECURRENT_STATE_WIDTH];
                if len > MAXIMUM_RECURRENT_STATE_WIDTH {
                    return Err(refuse(format!(
                        "{id} has recurrent state width {len}, above {MAXIMUM_RECURRENT_STATE_WIDTH}"
                    )));
                }
                for value in &mut state[..len] {
                    *value = held.f32()?;
                }
                match evaluated_raw {
                    Some((at, outputs, out_len)) => {
                        if len != state_width {
                            return Err(refuse(format!(
                                "{id} has recurrent state width {len}, expected {state_width}"
                            )));
                        }
                        HeldOutput::evaluated_recurrent_slice(
                            at,
                            &outputs[..out_len],
                            &state[..len],
                        )
                        .map_err(|error| refuse(format!("{id}'s held recurrent state: {error}")))?
                    }
                    None => {
                        if len != 0 {
                            return Err(refuse(format!(
                                "{id} holds no output but carries {len} recurrent state values"
                            )));
                        }
                        HeldOutput::INVALID
                    }
                }
            } else {
                match evaluated_raw {
                    Some((at, outputs, out_len)) => {
                        HeldOutput::evaluated_slice(at, &outputs[..out_len])
                            .map_err(|error| refuse(format!("{id}'s held output: {error}")))?
                    }
                    None => HeldOutput::INVALID,
                }
            };
            let (secretory_readiness, emission_cooldown_deadline) = if is_v2_chunk {
                let readiness = held.f32()?;
                if !readiness.is_finite() || !(0.0_f32..=1.0_f32).contains(&readiness) {
                    return Err(refuse(format!(
                        "{id} has secretory readiness {readiness}, outside [0, 1]"
                    )));
                }
                let deadline = Tick::new(held.u64()?);
                (readiness, deadline)
            } else {
                (INITIAL_SECRETORY_READINESS, Tick::new(0))
            };
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
                    secretory_readiness,
                    emission_cooldown_deadline,
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
        counted(chunk.items, ChunkFamily::Ledger, 1)?;
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

        let sources = initial_source_list;
        let scratch = Scratch::new(&config, &rules, sources.len(), &source_kernels)?;
        Ok(Self {
            config,
            rules,
            tick,
            epoch,
            events,
            field,
            population,
            sources,
            source_kernels,
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
            settle: Settle::default(),
            // A checkpoint holds no watch: a restored world watches nothing.
            watch: Watch::default(),
            // Nor any ancestry: a restored world traces none until told to.
            ancestry: None,
            restored: reader.held_bytes().saturating_add(window),
        })
    }
}
