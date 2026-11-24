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
//!   position, velocity and energy, its topology and parameter count, its
//!   heritable chemistry (affinity direction, the 33 motion-response
//!   coefficients `χ_e, χ_0..χ_31` and the sensing scale) and its held
//!   controller outputs, when it holds any;
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

use flife_core::checkpoint::ChunkFamily;
use flife_core::ids::OrganismId;
use flife_core::{CoreError, V2Feature};
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
    /// Enabled scientific `field-ecology-v2` feature slugs, sorted; omitted
    /// for `field-ecology-v1*` runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub v2_features: Option<Vec<&'static str>>,
    /// Whether this run satisfies `INV-08` (purely field-mediated interaction).
    pub inv08_pure_field: bool,
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
    /// Its motion-response coefficients: `χ_e` (energy) then `χ_0..χ_31`
    /// (the moment channels), in world-length² per second².
    pub motion_response: Vec<f64>,
    /// Its sensing scale in world units.
    pub sensing_scale: f64,
    /// The raw controller outputs it holds, as stored (36 `f32`s, widened
    /// exactly), and the tick they were evaluated at; none for a newborn
    /// that has not been evaluated yet.
    pub held: Option<InspectedHeld>,
    /// Secretory readiness `r_i ∈ [0, 1]` when `V2-04` `excitable-relay` is
    /// enabled; omitted on `field-ecology-v1*` and whenever `excitable-relay`
    /// is disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secretory_readiness: Option<f64>,
    /// Apical-basal unit polarity vector `p_i ∈ S^1` when `V2-08`
    /// `pcp-polarity` is enabled; omitted when disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polarity: Option<[f64; 2]>,
    /// Heritable lateral-belt polarity strength `α_pol ∈ [0, 1]` when `V2-08`
    /// `pcp-polarity` is enabled; omitted when disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polarity_strength: Option<f64>,
    /// Heritable spontaneous apical wedge curvature `β ∈ [-β_max, β_max]`
    /// when `V2-08` `pcp-polarity` is enabled; omitted when disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wedge_curvature: Option<f64>,
    /// Bounded epigenetic recurrent state `h ∈ [-1, 1]^S` (including unevaluated
    /// newborns carrying inherited state) when `V2-12` `epigenetic-recurrence`
    /// is enabled; omitted when disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epigenetic_state: Option<Vec<f64>>,
    /// Effective metabolic affinity `a_eff(h_0)` when `V2-12`
    /// `epigenetic-recurrence` with `affinity_rotation_enabled` is active;
    /// omitted when disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_affinity: Option<Vec<f64>>,
    /// Persisted kin-weighted mean neighbor `h̄_0 ∈ [-1, 1]` when `V2-12`
    /// `notch-delta` is enabled; omitted when disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_neighbor_h0: Option<f64>,
}

/// An organism's held controller outputs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InspectedHeld {
    /// The tick they were evaluated at.
    pub evaluated_at: u64,
    /// The raw outputs, in the action layout (`flife_neural::actions`).
    pub outputs: Vec<f64>,
    /// The bounded recurrent state carried into the next evaluation, as
    /// stored (`f32` widened exactly), for recurrent controller families
    /// (FL-054); omitted for stateless controllers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrent_state: Option<Vec<f64>>,
}

/// The field's energy plane (plus optional `field-ecology-v2` trail energy and
/// vascular conductance planes when enabled).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InspectedField {
    /// `field`.
    pub record: &'static str,
    /// The grid's cells, `[x, y]`.
    pub cells: [u32; 2],
    /// Each cell's Carrier-0 energy, row-major: cell `(x, y)` at `y · cells_x + x`.
    pub energy: Vec<f64>,
    /// Each cell's Carrier-1 trail energy (`e_1`), row-major, when `V2-15`
    /// (`dual-carrier-trails`) is enabled; omitted on single-carrier runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trail_energy: Option<Vec<f64>>,
    /// Each cell's mean incident vascular edge conductance `g_e`, row-major,
    /// when `V2-14` (`vascular-field`) is enabled; omitted when disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vascular_conductance: Option<Vec<f64>>,
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
    /// The weight dtype its genome stores (`f32` or `f16`); each value
    /// below is one of that dtype's, written as the F32 it widens to
    /// exactly.
    pub dtype: &'static str,
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
    // The genome chunk is streamed, never held whole; the container's
    // digest is the store's, hashed from its files a buffer at a time.
    let recovered =
        store.latest_valid_streaming(ChunkFamily::Genomes, |sequence, held, genomes| {
            let restored = restorable(held, genomes, profile, revision, session)?;
            let tick = restored.world.tick().get();
            if options.at.is_some_and(|at| at != tick) {
                return Err(conflict(format!("it holds tick {tick}")));
            }
            Ok((restored.world, store.verify(sequence)?))
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
    // FL-043: admit the restored world against `[execution] maximum_total_bytes`.
    world.admit_memory(false, 0, 0)?;
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
            dtype: dump.dtype.name(),
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
    let excitable_active = world
        .config()
        .config()
        .v2
        .as_ref()
        .is_some_and(|v2| v2.is_enabled(V2Feature::ExcitableRelay));
    let polarity_active = world
        .config()
        .config()
        .v2
        .as_ref()
        .is_some_and(|v2| v2.is_enabled(V2Feature::PcpPolarity));
    let differentiation = world.v2_rules().differentiation();
    let affinity_rotation_active = world.v2_rules().affinity_rotation_enabled();
    let notch_delta_active = world.v2_rules().notch_delta().is_some();
    let population = world.population();
    let mut organisms = Vec::new();
    let mut organism_energy = 0.0;
    for organism in population.iter() {
        let genome = population
            .genome(organism.id)
            .ok_or_else(|| conflict(format!("{} has no genome", organism.id)))?;
        organism_energy += organism.energy;
        let epigenetic_state = differentiation.map(|diff| {
            let state = organism.held.recurrent_state();
            if state.is_empty() {
                vec![0.0_f64; diff.state_width]
            } else {
                state.iter().map(|&value| f64::from(value)).collect()
            }
        });
        let effective_affinity = if affinity_rotation_active {
            world
                .effective_affinity(organism.id)?
                .map(|direction| direction.to_vec())
        } else {
            None
        };
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
            motion_response: genome.traits().motion_response().to_vec(),
            sensing_scale: genome.traits().sensing_scale(),
            held: organism
                .held
                .outputs()
                .zip(organism.held.evaluated_at())
                .map(|(outputs, tick)| {
                    let state = organism.held.recurrent_state();
                    InspectedHeld {
                        evaluated_at: tick.get(),
                        outputs: outputs.iter().map(|&value| f64::from(value)).collect(),
                        recurrent_state: (!state.is_empty())
                            .then(|| state.iter().map(|&value| f64::from(value)).collect()),
                    }
                }),
            secretory_readiness: excitable_active.then(|| f64::from(organism.secretory_readiness)),
            polarity: polarity_active.then_some(organism.polarity),
            polarity_strength: polarity_active
                .then(|| genome.traits().polarity_strength())
                .flatten(),
            wedge_curvature: polarity_active
                .then(|| genome.traits().wedge_curvature())
                .flatten(),
            epigenetic_state,
            effective_affinity,
            last_neighbor_h0: notch_delta_active.then(|| f64::from(organism.last_neighbor_h0)),
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
        v2_features: world.config().enabled_v2_features(),
        inv08_pure_field: world.config().inv08_pure_field(),
        world: InspectedExtent {
            width: grid.world_width(),
            height: grid.world_height(),
            cells,
        },
        live: organisms.len(),
        field_energy: state.energy_total(),
        organism_energy,
    };
    let field = options.field.then(|| {
        let trail_energy = state
            .trail_energy()
            .map(|plane| plane.iter().map(|&cell| f64::from(cell)).collect());
        let vascular_conductance = state.vascular_edges().map(|(gx, gy)| {
            let nx = grid.cells_x() as usize;
            let ny = grid.cells_y() as usize;
            let mut out = Vec::with_capacity(nx.saturating_mul(ny));
            for y in 0..ny {
                let south_y = if y == 0 { ny - 1 } else { y - 1 };
                for x in 0..nx {
                    let west_x = if x == 0 { nx - 1 } else { x - 1 };
                    let c = y * nx + x;
                    let west = y * nx + west_x;
                    let south = south_y * nx + x;
                    let mean = 0.25
                        * (f64::from(gx[c])
                            + f64::from(gx[west])
                            + f64::from(gy[c])
                            + f64::from(gy[south]));
                    out.push(mean);
                }
            }
            out
        });
        InspectedField {
            record: "field",
            cells,
            energy: state.energy().iter().map(|&cell| f64::from(cell)).collect(),
            trail_energy,
            vascular_conductance,
        }
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
