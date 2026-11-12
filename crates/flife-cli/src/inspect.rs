//! Inspection of a run directory's committed state (plan §14.6's
//! `inspect`, §15.1's raw state; FL-048's H1 and H4).
//!
//! [`inspect`] restores one checkpoint of a run directory (the newest that
//! restores, or the one at the requested tick) and reports its committed
//! world as records:
//! - one [`Inspected`] header: the checkpoint, the world's digest, its size
//!   and grid, the live count and the energy the field and the organisms
//!   hold;
//! - one [`InspectedOrganism`] per live organism, in identity order: its
//!   identity, genome, lineage, generation, parent and birth tick, its
//!   position, velocity and energy, its topology and parameter count, and
//!   its genetic affinity direction;
//! - with `field`, one [`InspectedField`]: the energy plane, row-major
//!   (`y` outer, `x` inner), each cell's `f32` widened exactly;
//! - with `weights`, one [`InspectedWeights`] per organism named, in the
//!   order named (plan §14.2's on-demand weight dump): its whole controller,
//!   each layer's `weights[out][in]` and `biases[out]` as stored.
//!
//! Every number is the stored value: nothing is resampled or summarized.
//! The run directory is checked (its configuration and catalog) before its
//! store is opened, and the store is opened as `flife resume` and `flife
//! branch` open it: as its writer, so a run another process holds is
//! refused and a crashed writer's temporary files are cleared. Nothing
//! else is written.

use std::path::Path;

use flife_core::CoreError;
use flife_core::canonical::Sha256Digest;
use flife_core::ids::OrganismId;
use flife_engine::{ControllerWeights, NumericalProfile, World};
use flife_store::{ChunkStore, Durability};
use serde::Serialize;

use crate::persist::{STORE_DIR, read_config, restorable, run_identity};

/// The inspection records' schema.
pub const INSPECT_SCHEMA: &str = "frankenlife.inspect.v1";

/// What to inspect.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InspectOptions {
    /// The committed tick of the checkpoint; the newest that restores if
    /// `None`.
    pub at: Option<u64>,
    /// Whether to report the field's energy plane.
    pub field: bool,
    /// The organisms whose whole controller to report, in this order.
    pub weights: Vec<u64>,
}

/// The checkpoint an inspection restored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InspectedCheckpoint {
    /// Its sequence number in the store.
    pub sequence: u64,
    /// Its committed tick.
    pub tick: u64,
    /// Its container's SHA-256.
    pub sha256: String,
}

/// The world's extent.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct InspectedExtent {
    /// The world's width in world units.
    pub width: f64,
    /// Its height.
    pub height: f64,
    /// The field grid's cells, `[x, y]`.
    pub cells: [u32; 2],
}

/// The inspection's header record.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Inspected {
    /// `inspect`.
    pub record: &'static str,
    /// [`INSPECT_SCHEMA`].
    pub schema: &'static str,
    /// The run directory, as given.
    pub run_dir: String,
    /// The checkpoint restored.
    pub checkpoint: InspectedCheckpoint,
    /// The committed world's digest.
    pub digest: String,
    /// The model it runs.
    pub model: String,
    /// The world's extent and grid.
    pub world: InspectedExtent,
    /// Live organisms.
    pub live: usize,
    /// Energy the field holds.
    pub field_energy: f64,
    /// Energy the organisms hold.
    pub organism_energy: f64,
}

/// One live organism as stored.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InspectedOrganism {
    /// `organism`.
    pub record: &'static str,
    /// Its stable identity.
    pub id: u64,
    /// Its genome's identity.
    pub genome: u64,
    /// Its founder lineage.
    pub lineage: u64,
    /// Its generation.
    pub generation: u64,
    /// Its parent; none for a founder.
    pub parent: Option<u64>,
    /// The first committed tick it exists in.
    pub birth_tick: u64,
    /// Its wrapped position.
    pub position: [f64; 2],
    /// Its velocity.
    pub velocity: [f64; 2],
    /// Its energy.
    pub energy: f64,
    /// Its topology's index in the catalog.
    pub topology: usize,
    /// Its controller's parameter count.
    pub parameters: usize,
    /// Its genetic affinity direction.
    pub affinity: Vec<f64>,
}

/// The field's energy plane.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InspectedField {
    /// `field`.
    pub record: &'static str,
    /// The grid's cells, `[x, y]`.
    pub cells: [u32; 2],
    /// Each cell's energy, row-major: cell `(x, y)` at `y · cells_x + x`.
    pub energy: Vec<f64>,
}

/// One organism's whole controller (plan §14.2's on-demand weight dump):
/// its own genome's parameters as stored, by layer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InspectedWeights {
    /// `weights`.
    pub record: &'static str,
    /// Its stable identity.
    pub id: u64,
    /// Its genome's identity.
    pub genome: u64,
    /// Its topology's index in the catalog.
    pub topology: usize,
    /// Its controller's parameter count.
    pub parameters: usize,
    /// Its layers in evaluation order: the 104 sensor inputs feed the
    /// first, `tanh` follows each hidden layer, and the last gives the 36
    /// raw outputs linearly.
    pub layers: Vec<InspectedLayer>,
}

/// One dense layer, `z = bias + weights · x`, each `f32` as stored.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InspectedLayer {
    /// Its fan-in.
    pub inputs: usize,
    /// Its fan-out.
    pub outputs: usize,
    /// `weights[out][in]`: one row of `inputs` per output.
    pub weights: Vec<Vec<f32>>,
    /// `biases[out]`.
    pub biases: Vec<f32>,
}

/// An inspection: the header, the organisms, the field and the controllers
/// asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Inspection {
    /// The header.
    pub inspected: Inspected,
    /// The live organisms, in identity order.
    pub organisms: Vec<InspectedOrganism>,
    /// The field's energy plane, with [`InspectOptions::field`].
    pub field: Option<InspectedField>,
    /// The controllers [`InspectOptions::weights`] names, in its order.
    pub weights: Vec<InspectedWeights>,
}

fn conflict(detail: String) -> CoreError {
    CoreError::StateConflict {
        operation: "inspect run",
        detail,
    }
}

/// Inspect the run in `run_dir` at the checkpoint `options` names.
///
/// # Errors
/// As [`read_config`] and the catalog for a directory that holds no run;
/// [`CoreError::StateConflict`] when no checkpoint restores (at the
/// requested tick, if one is given), another process holds the run, or an
/// organism whose weights are asked for is not in the restored world; the
/// store's errors.
pub fn inspect(run_dir: &Path, options: InspectOptions) -> Result<Inspection, CoreError> {
    let config = read_config(run_dir)?;
    let revision = config.config_hash();
    let (session, _) = run_identity(run_dir, revision)?;
    let profile = NumericalProfile::reference_f32()?;
    let store = ChunkStore::open(run_dir.join(STORE_DIR), Durability::ProcessCrash)?;
    let recovered = store.latest_valid(|_, bytes| {
        let restored = restorable(bytes, profile, revision, session)?;
        let tick = restored.world.tick().get();
        if options.at.is_some_and(|at| at != tick) {
            return Err(conflict(format!("it holds tick {tick}")));
        }
        Ok((restored.world, Sha256Digest::of(bytes)))
    })?;
    let Some((sequence, (world, sha256))) = recovered.latest else {
        return Err(conflict(format!(
            "{} holds no checkpoint that restores{}",
            run_dir.display(),
            options
                .at
                .map_or_else(String::new, |at| format!(" at tick {at}"))
        )));
    };
    let checkpoint = InspectedCheckpoint {
        sequence,
        tick: world.tick().get(),
        sha256: sha256.to_hex(),
    };
    of(run_dir, checkpoint, &world, &options)
}

/// `id`'s whole controller in committed `world`.
fn weights(world: &World, id: u64) -> Result<InspectedWeights, CoreError> {
    let dump = world
        .organism_weights(OrganismId::new(id))?
        .ok_or_else(|| {
            conflict(format!(
                "organism {id} is not in the committed world at tick {}",
                world.tick().get()
            ))
        })?;
    InspectedWeights::of(&dump)
}

impl InspectedWeights {
    /// The record of `dump`, its parameters laid out by layer: what
    /// `flife inspect --weights` writes, and the Studio's weight dump
    /// (FL-021) too.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when a layer's shape does not lay out
    /// the parameters, which a dump from [`World::organism_weights`] never
    /// has.
    pub fn of(dump: &ControllerWeights) -> Result<Self, CoreError> {
        let id = dump.organism.get();
        let mut layers = Vec::with_capacity(dump.layers.len());
        for (index, shape) in dump.layers.iter().enumerate() {
            let missing = || CoreError::StateConflict {
                operation: "dump organism weights",
                detail: format!("organism {id}'s layer {index} is not in its parameters"),
            };
            let (weights, biases) = dump.layer(index).ok_or_else(missing)?;
            if shape.inputs == 0 {
                return Err(missing());
            }
            layers.push(InspectedLayer {
                inputs: shape.inputs,
                outputs: shape.outputs,
                weights: weights
                    .chunks_exact(shape.inputs)
                    .map(<[f32]>::to_vec)
                    .collect(),
                biases: biases.to_vec(),
            });
        }
        Ok(Self {
            record: "weights",
            id,
            genome: dump.genome.get(),
            topology: dump.topology,
            parameters: dump.parameters.len(),
            layers,
        })
    }
}

/// The records of committed `world`, restored from `checkpoint`.
fn of(
    run_dir: &Path,
    checkpoint: InspectedCheckpoint,
    world: &World,
    options: &InspectOptions,
) -> Result<Inspection, CoreError> {
    let population = world.population();
    let mut organisms = Vec::new();
    let mut organism_energy = 0.0;
    for organism in population.iter() {
        let genome = population
            .genome(organism.id)
            .ok_or_else(|| conflict(format!("{} has no genome", organism.id)))?;
        organism_energy += organism.energy;
        organisms.push(InspectedOrganism {
            record: "organism",
            id: organism.id.get(),
            genome: organism.genome.get(),
            lineage: organism.lineage.get(),
            generation: u64::from(organism.generation.get()),
            parent: organism.parent.map(|parent| parent.get()),
            birth_tick: organism.birth_tick.get(),
            position: organism.position,
            velocity: organism.velocity,
            energy: organism.energy,
            topology: genome.topology().index(),
            parameters: genome.parameters().len(),
            affinity: genome.traits().affinity().to_vec(),
        });
    }
    let state = world.field();
    let grid = state.grid();
    let cells = [grid.cells_x(), grid.cells_y()];
    let inspected = Inspected {
        record: "inspect",
        schema: INSPECT_SCHEMA,
        run_dir: run_dir.display().to_string(),
        checkpoint,
        digest: world.digest().to_hex(),
        model: world.config().config().model.clone(),
        world: InspectedExtent {
            width: grid.world_width(),
            height: grid.world_height(),
            cells,
        },
        live: organisms.len(),
        field_energy: state.energy_total(),
        organism_energy,
    };
    let field = options.field.then(|| InspectedField {
        record: "field",
        cells,
        energy: state.energy().iter().map(|&cell| f64::from(cell)).collect(),
    });
    let weights = options
        .weights
        .iter()
        .map(|&id| weights(world, id))
        .collect::<Result<_, _>>()?;
    Ok(Inspection {
        inspected,
        organisms,
        field,
        weights,
    })
}
