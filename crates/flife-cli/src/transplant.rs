//! Transplants (FL-042; plan §14.5): a local package grafted into a host
//! world as it is founded, made a new run directory.
//!
//! [`transplant`] reads and verifies a package ([`read_package`]), founds
//! the host from its configuration, grafts the package's disk into it
//! ([`World::graft`]) with the disk's centre moved to the requested
//! position, snapped to whole cells so that cells land on cells, and writes
//! the grafted world as the founding checkpoint of a new run directory.
//! `flife resume` continues it.
//!
//! - The run directory's configuration is the host's, and everything
//!   outside the disk is the host's: its field, sources, organisms and
//!   identity allocators. The package's field cells replace the host's
//!   under them, every plane; the host's founders inside the disk are
//!   displaced; the package's organisms enter as founders with the host's
//!   next identities, kin kept as kin. The ledger opens at the grafted
//!   world's holdings: the host's, less what was displaced, plus the
//!   package's endowment.
//! - The grafted world is not the world its configuration founds, so the
//!   run has no founding world to fall back to. Its catalog records a
//!   transplant session ([`transplant_session`]: the package container's
//!   SHA-256, the host configuration's hash, the offset and the name), never
//!   [`RUN_SESSION`], and [`crate::persist::recover`] refuses a run of
//!   another session none of whose checkpoints restores.
//! - `transplant.json` in the run directory holds the record the command
//!   emits: the package, the host, what the graft did, the founding
//!   checkpoint and the memory admission. It is written last, to a
//!   temporary name renamed into place.
//! - Memory (FL-043) is admitted as a run's is, against the host's
//!   `[execution] maximum_total_bytes`: the host world's terms, the
//!   founding checkpoint's (streamed, `pause-and-stream`, whatever the
//!   configuration's mode) and, as the package term, the package's restored
//!   world ([`World::held_bytes`]), the container it was restored from, and
//!   what the graft makes before it releases the host's parts
//!   ([`World::graft_bytes`]: about a second host world). Reading holds the
//!   container and the package's world at once, before the host is
//!   founded; the graft holds that world, the host's and its own parts.
//!   The sum bounds both. No controller backend is installed: the
//!   transplant takes no step.
//!
//! Every refusal (a position outside the host, a run directory that holds
//! anything, a checkpoint the host's budget cannot take, a package that
//! does not verify, a host whose cells, topology catalog or parameter type
//! differ, a disk that wraps onto itself in a narrower host, a package
//! organism the host's limits refuse, a population or genome quota too
//! small, a memory budget too small for the host and the package together)
//! comes before anything is written. A failure while the run directory is
//! written (the host refusing a write, or the founding checkpoint's
//! encoding) leaves no run that silently refounds: before the catalog exists the
//! directory is not a run directory, and after it, with no checkpoint that
//! restores, `flife resume` refuses it; once the founding checkpoint is
//! published it is a run that resumes, missing only `transplant.json`.
//!
//! [`World::graft`]: flife_engine::World::graft
//! [`World::held_bytes`]: flife_engine::World::held_bytes
//! [`World::graft_bytes`]: flife_engine::World::graft_bytes

use std::path::Path;

use flife_core::canonical::Sha256Digest;
use flife_core::ids::SessionId;
use flife_core::memory::total;
use flife_core::{CoreError, Holdings, ValidatedConfig};
use flife_engine::NumericalProfile;
use flife_runtime::{CommandBook, Journal as _, SessionConfig};
use flife_store::CommandJournal;
use serde::Serialize;

use crate::branch::BRANCH_NAME_MAX_BYTES;
use crate::checkpointing::{self, CheckpointMode};
use crate::discovery::publish;
use crate::package::{PackageSource, read_package};
use crate::persist::{
    CATALOG_FILE, Checkpoints, Persistence, StoreDurability, create, on_plain_thread, vacant,
};
use crate::run::{CheckpointAt, RUN_SCHEMA, RUN_SESSION, Record, found};

/// The record a transplant run directory keeps.
pub const TRANSPLANT_FILE: &str = "transplant.json";

/// What to transplant, and where.
#[derive(Debug, Clone, PartialEq)]
pub struct TransplantOptions {
    /// Where the package's disk centre goes in the host: a world position,
    /// snapped to the nearest whole-cell move.
    pub at: [f64; 2],
    /// The transplant's name, part of its identity; the new directory's
    /// name if `None`.
    pub name: Option<String>,
    /// How the founding checkpoint and the record are written.
    pub durability: StoreDurability,
}

/// The package a transplant grafted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TransplantPackage {
    /// Its directory, as given.
    pub dir: String,
    /// Its world container's SHA-256.
    pub sha256: String,
    /// The cropped world's digest.
    pub digest: String,
    /// Where it was cut from.
    pub source: PackageSource,
}

/// The host a transplant grafted into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TransplantHost {
    /// Its configuration's canonical hash: the run's.
    pub config_hash: String,
    /// Its founders.
    pub founders: u64,
    /// The founded host's digest, before the graft.
    pub digest: String,
}

/// Energy held, as a record reports it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct HeldEnergy {
    /// In the field.
    pub field: f64,
    /// In organisms.
    pub organisms: f64,
    /// Their sum.
    pub total: f64,
}

impl From<Holdings> for HeldEnergy {
    fn from(holdings: Holdings) -> Self {
        Self {
            field: holdings.field(),
            organisms: holdings.organisms(),
            total: holdings.total(),
        }
    }
}

/// What the graft did.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct GraftRecord {
    /// The position asked for.
    pub at: [f64; 2],
    /// Where the disk's centre went: the package's centre moved by
    /// `offset` whole cells, wrapped.
    pub center: [f64; 2],
    /// The disk's radius in world units.
    pub radius: f64,
    /// Whole cells the disk moved by, in x and y.
    pub offset: [i64; 2],
    /// Host cells replaced.
    pub cells: u64,
    /// The energy they held.
    pub displaced_field: f64,
    /// Host founders displaced.
    pub displaced_organisms: u64,
    /// Their energy.
    pub displaced_organism_energy: f64,
    /// Organisms grafted.
    pub organisms: u64,
    /// The new lineages they form.
    pub lineages: u64,
    /// The host's holdings before the graft.
    pub before: HeldEnergy,
    /// The package's endowment.
    pub endowment: HeldEnergy,
    /// The grafted world's holdings.
    pub after: HeldEnergy,
}

/// The session a transplant named `name` runs: the first eight bytes,
/// little-endian, of the SHA-256 of `frankenlife.transplant-session.v1`, a
/// zero byte, the package container's SHA-256 (hex), the host
/// configuration's hash, the offset's two little-endian `i64`s and the name.
#[must_use]
pub fn transplant_session(
    package: &str,
    host: Sha256Digest,
    offset: [i64; 2],
    name: &str,
) -> SessionId {
    let mut input = b"frankenlife.transplant-session.v1\0".to_vec();
    input.extend_from_slice(package.as_bytes());
    input.extend_from_slice(host.as_bytes());
    for step in offset {
        input.extend_from_slice(&step.to_le_bytes());
    }
    input.extend_from_slice(name.as_bytes());
    let digest = Sha256Digest::of(&input);
    let mut first = [0_u8; 8];
    first.copy_from_slice(&digest.as_bytes()[..8]);
    SessionId::new(u64::from_le_bytes(first))
}

fn conflict(detail: String) -> CoreError {
    CoreError::StateConflict {
        operation: "transplant package",
        detail,
    }
}

/// Graft the package in `package_dir` into a host founded from `host`, at
/// `options.at`, as the new (or empty) run directory `run_dir` (see the
/// [module documentation](self)), returning the [`Record::Transplant`] that
/// describes it.
///
/// # Errors
/// [`CoreError::StateConflict`] for a name that is empty or too long, a
/// position outside the host's world, `run_dir` holding a run or other
/// files, and [`read_package`]'s and [`World::graft`]'s refusals;
/// [`CoreError::NumericalInvalid`] for a disk the host's measurement
/// refuses or a package organism the host's limits refuse;
/// [`CoreError::BudgetExhausted`] when the host cannot take the grafted
/// organisms, or its memory budget cannot take its founding checkpoint, or
/// its world and the package's together (FL-043); the store's and
/// catalog's errors.
///
/// [`World::graft`]: flife_engine::World::graft
pub fn transplant(
    package_dir: &Path,
    host: &ValidatedConfig,
    run_dir: &Path,
    options: &TransplantOptions,
) -> Result<Record, CoreError> {
    let name = match &options.name {
        Some(name) => name.clone(),
        None => run_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    if name.is_empty() || name.len() > BRANCH_NAME_MAX_BYTES {
        return Err(conflict(format!(
            "a transplant name is 1 to {BRANCH_NAME_MAX_BYTES} bytes, not {}",
            name.len()
        )));
    }
    // The cheap refusals first: the host's sides are its configuration's.
    let world_shape = &host.config().world;
    let side = [world_shape.width, world_shape.height];
    if !(0..2)
        .all(|axis| options.at[axis].is_finite() && (0.0..side[axis]).contains(&options.at[axis]))
    {
        return Err(conflict(format!(
            "{:?} lies outside the host's {} × {} world",
            options.at, side[0], side[1]
        )));
    }
    vacant(run_dir)?;
    // The founding checkpoint is streamed whatever the configuration's
    // mode. Its term, then the whole peak (FL-043), are admitted before the
    // run directory is made, as a run's are.
    let persistence = Persistence {
        checkpoint_every: 0,
        durability: options.durability,
        mode: CheckpointMode::PauseAndStream,
        ..Persistence::new(run_dir, host)
    };
    let checkpointing = checkpointing::admit(host, persistence.mode, persistence.retained_bytes)?;
    let (package, crop) = read_package(package_dir, NumericalProfile::reference_f32()?)?;
    let mut world = found(host)?;
    let memory = world.admit_memory_beside(
        false,
        checkpointing.peak_bytes,
        0,
        total([
            crop.held_bytes(),
            package.world.bytes,
            world.graft_bytes(&crop),
        ]),
    )?;
    let grid = *world.field().grid();
    let disk = package.crop.disk();
    let cell = [grid.cell_width(), grid.cell_height()];
    // Bounded by the two worlds' sides over a cell: exact as an i64.
    let offset =
        [0, 1].map(|axis| ((options.at[axis] - disk.center[axis]) / cell[axis]).round() as i64);
    let host_digest = world.digest();
    let founders = u64::try_from(world.population().len()).unwrap_or(u64::MAX);
    let graft = world.graft(&crop, disk.center, disk.radius, offset)?;
    drop(crop);

    let revision = host.config_hash();
    let session = transplant_session(&package.world.sha256, revision, offset, &name);
    if session == RUN_SESSION {
        return Err(conflict(format!(
            "the transplant's session is {RUN_SESSION}, a founded run's: rename it"
        )));
    }
    let limits = SessionConfig::from_config(session, host)?;
    let mut book = CommandBook::new(session, revision, limits.command_capacity);
    book.set_durability(limits.durability);

    create(run_dir, host)?;
    let catalog = run_dir.join(CATALOG_FILE);
    on_plain_thread("record the transplant", || {
        CommandJournal::open(&catalog)?
            .open_run(session, revision)
            .map(|_| ())
    })?;
    let mut checkpoints = Checkpoints::open(&persistence, host, session)?;
    let published = checkpoints.stream_from(&world, &book)?;
    let Record::Checkpoint {
        sequence,
        sha256,
        bytes,
        ..
    } = published
    else {
        return Err(conflict("the founding checkpoint was not published".into()));
    };
    let record = Record::Transplant {
        schema: RUN_SCHEMA,
        name,
        session: session.get(),
        package: TransplantPackage {
            dir: package_dir.display().to_string(),
            sha256: package.world.sha256,
            digest: package.world.digest,
            source: package.source,
        },
        host: TransplantHost {
            config_hash: revision.to_hex(),
            founders,
            digest: host_digest.to_hex(),
        },
        graft: GraftRecord {
            at: options.at,
            center: graft.center,
            radius: graft.radius,
            offset: graft.offset,
            cells: graft.cells,
            displaced_field: graft.displaced_field,
            displaced_organisms: graft.displaced_organisms,
            displaced_organism_energy: graft.displaced_organism_energy,
            organisms: graft.organisms,
            lineages: graft.lineages,
            before: graft.before.into(),
            endowment: graft.endowment.into(),
            after: graft.after.into(),
        },
        checkpoint: CheckpointAt {
            sequence,
            tick: world.tick().get(),
        },
        sha256,
        bytes,
        digest: world.digest().to_hex(),
        memory: Box::new(memory),
    };
    let mut text =
        serde_json::to_string_pretty(&record).map_err(|error| CoreError::StateConflict {
            operation: "encode transplant",
            detail: error.to_string(),
        })?;
    text.push('\n');
    publish(
        run_dir,
        TRANSPLANT_FILE,
        text.as_bytes(),
        options.durability,
    )?;
    Ok(record)
}
