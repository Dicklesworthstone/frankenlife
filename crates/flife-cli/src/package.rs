//! Local transplant packages (FL-042; plan §14.5): one disk of a durable
//! checkpoint, with its cropped field, its declared boundary condition and
//! its energy endowment, written into a discovery bundle's `package/`.
//!
//! | File | Holds |
//! |---|---|
//! | `world.ckpt` | the crop ([`World::cropped`]) as a world checkpoint container: the world's chunks, no command chunk |
//! | `package.json` | the manifest ([`PACKAGE_SCHEMA`]): the source, the crop, the boundary, the endowment, the container's size, SHA-256 and digest |
//!
//! Local-package semantics, against the full-world export's:
//!
//! - The full world (`world/`) is the whole checkpoint as a run. Resumed,
//!   it reaches the state the source run reaches.
//! - The package holds only the disk: the organisms a removal of it would
//!   take, each with its row, held state and genome, and the field cells
//!   whose centres lie in it, every plane. Everything else in the
//!   container is zero or empty.
//! - Its boundary condition is declared: `exterior = "host"`. Nothing
//!   outside the crop travels with it: not the field around it, the
//!   sources, the neighbouring organisms or the run's scheduled commands.
//!   Whatever receives it supplies all of those. It is not a run, and it
//!   does not resume alone.
//! - Its endowment is the energy it carries: the cropped field's and its
//!   organisms', the crop's ledger holdings.
//!
//! [`read_package`] verifies a package before anything uses it: the
//! manifest's schema and boundary, the container's size and SHA-256, the
//! world's restore (which checks its ledger against its state bit for bit),
//! its digest, tick, configuration and organism count, and the manifest's
//! endowment against the restored holdings.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use flife_core::CoreError;
use flife_core::canonical::Sha256Digest;
use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_engine::{NumericalProfile, WORLD_CHECKPOINT_CHUNKS, World};
use serde::{Deserialize, Serialize};

use crate::discovery::{Disk, storage, write_new};
use crate::persist::StoreDurability;

/// The manifest's schema.
pub const PACKAGE_SCHEMA: &str = "frankenlife.package.v1";

/// The package's directory in a discovery bundle.
pub const PACKAGE_DIR: &str = "package";

/// The manifest's file name.
pub const PACKAGE_FILE: &str = "package.json";

/// The world container's file name.
pub const PACKAGE_WORLD_FILE: &str = "world.ckpt";

/// The one boundary condition a package declares: whatever receives it
/// supplies everything outside the crop.
pub const HOST_EXTERIOR: &str = "host";

/// The longest manifest read back.
const MANIFEST_MAX_BYTES: u64 = 1 << 16;

/// The largest container read back.
const WORLD_MAX_BYTES: u64 = 1 << 34;

/// Where a package was cut from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSource {
    /// The source run directory, as given.
    pub run_dir: String,
    /// Its session.
    pub session: u64,
    /// The checkpoint's sequence number in its store.
    pub sequence: u64,
    /// The checkpoint's committed tick.
    pub tick: u64,
    /// The checkpoint container's SHA-256.
    pub checkpoint_sha256: String,
    /// The run's configuration hash.
    pub config_sha256: String,
}

/// The crop: a disk of the source world and the field cells it holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crop {
    /// The disk's centre: a world position.
    pub center: [f64; 2],
    /// Its radius in world units.
    pub radius: f64,
    /// The centre's and radius's IEEE-754 bits, which JSON carries exactly
    /// (decimal text may come back a unit in the last place off). The disk
    /// is these.
    pub center_bits: [u64; 2],
    /// See `center_bits`.
    pub radius_bits: u64,
    /// The field cells whose centres lie in it.
    pub cells: u64,
    /// The source field's cells in x and y.
    pub grid: [u32; 2],
    /// A cell's width and height in world units.
    pub cell_size: [f64; 2],
}

impl Crop {
    /// The disk, exactly: from its bits.
    #[must_use]
    pub fn disk(&self) -> Disk {
        Disk {
            center: self.center_bits.map(f64::from_bits),
            radius: f64::from_bits(self.radius_bits),
        }
    }
}

/// The declared boundary condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Boundary {
    /// What lies outside the crop: [`HOST_EXTERIOR`].
    pub exterior: String,
}

/// The energy a package carries.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endowment {
    /// The cropped field's energy.
    pub field: f64,
    /// The organisms' energy.
    pub organisms: f64,
    /// Their sum.
    pub total: f64,
}

/// The package's world container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageWorld {
    /// Its file, [`PACKAGE_WORLD_FILE`].
    pub path: String,
    /// Its size.
    pub bytes: u64,
    /// Its SHA-256.
    pub sha256: String,
    /// The cropped world's digest.
    pub digest: String,
}

/// A package's manifest, `package.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    /// [`PACKAGE_SCHEMA`].
    pub schema: String,
    /// Where it was cut from.
    pub source: PackageSource,
    /// The crop.
    pub crop: Crop,
    /// The declared boundary condition.
    pub boundary: Boundary,
    /// The energy it carries.
    pub endowment: Endowment,
    /// Its organisms.
    pub organisms: u64,
    /// Their distinct founder lineages.
    pub lineages: u64,
    /// Its world container.
    pub world: PackageWorld,
}

fn refuse(detail: impl Into<String>) -> CoreError {
    CoreError::StateConflict {
        operation: "read package",
        detail: detail.into(),
    }
}

/// Cut the disk `disk` of committed `world`, which `source` names, into the
/// new directory `dir` (see the [module documentation](self)), and return
/// its manifest.
///
/// # Errors
/// As [`World::cropped`] and [`World::write_checkpoint`];
/// [`CoreError::StorageFailed`] when `dir` exists or a file cannot be
/// written.
pub fn write_package(
    dir: &Path,
    world: &World,
    disk: Disk,
    source: PackageSource,
    durability: StoreDurability,
) -> Result<Package, CoreError> {
    let (crop, measurement) = world.cropped(disk.center, disk.radius)?;
    let mut writer = CheckpointWriter::new(&crop.config().config().model);
    crop.write_checkpoint(&mut writer)?;
    let bytes = writer.finish()?;
    let holdings = crop.ledger().holdings();
    let grid = crop.field().grid();
    let package = Package {
        schema: PACKAGE_SCHEMA.into(),
        source,
        crop: Crop {
            center: measurement.center,
            radius: measurement.radius,
            center_bits: measurement.center.map(f64::to_bits),
            radius_bits: measurement.radius.to_bits(),
            cells: measurement.cells,
            grid: [grid.cells_x(), grid.cells_y()],
            cell_size: [grid.cell_width(), grid.cell_height()],
        },
        boundary: Boundary {
            exterior: HOST_EXTERIOR.into(),
        },
        endowment: Endowment {
            field: holdings.field(),
            organisms: holdings.organisms(),
            total: holdings.total(),
        },
        organisms: measurement.organisms,
        lineages: measurement.lineages,
        world: PackageWorld {
            path: PACKAGE_WORLD_FILE.into(),
            bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            sha256: Sha256Digest::of(&bytes).to_hex(),
            digest: crop.digest().to_hex(),
        },
    };
    drop(crop);
    let mut text =
        serde_json::to_string_pretty(&package).map_err(|error| CoreError::StateConflict {
            operation: "encode package",
            detail: error.to_string(),
        })?;
    text.push('\n');
    fs::create_dir(dir).map_err(|error| storage("create package directory", dir, &error))?;
    write_new(&dir.join(PACKAGE_WORLD_FILE), &bytes, durability)?;
    write_new(&dir.join(PACKAGE_FILE), text.as_bytes(), durability)?;
    if durability == StoreDurability::PowerLoss {
        File::open(dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| storage("flush package directory", dir, &error))?;
    }
    Ok(package)
}

/// At most `limit` bytes of `path`.
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, CoreError> {
    let file = File::open(path).map_err(|error| storage("open package file", path, &error))?;
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| storage("read package file", path, &error))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(refuse(format!(
            "{} is longer than {limit} bytes",
            path.display()
        )));
    }
    Ok(bytes)
}

/// Whether `manifest` agrees with `restored` to within a few units in the
/// last place: JSON carries the endowment as decimal text.
fn agrees(manifest: f64, restored: f64) -> bool {
    (manifest - restored).abs() <= 4.0 * f64::EPSILON * restored.abs()
}

/// Read and verify the package in `dir` under `profile` (see the [module
/// documentation](self)): its manifest and its restored world.
///
/// # Errors
/// [`CoreError::StateConflict`] for a directory without the manifest and
/// the container, or a manifest that does not parse, has another schema,
/// boundary or container name, or disagrees with the container or the
/// restored world; [`CoreError::StorageFailed`] for a file that cannot be
/// read; and the restore's [`CoreError::IncompatibleCheckpoint`].
pub fn read_package(dir: &Path, profile: NumericalProfile) -> Result<(Package, World), CoreError> {
    for name in [PACKAGE_FILE, PACKAGE_WORLD_FILE] {
        let path = dir.join(name);
        // Absent (or under something that is not a directory) means this is
        // not a package; any other failure is the host's.
        let present = match fs::metadata(&path) {
            Ok(metadata) => metadata.is_file(),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                false
            }
            Err(error) => return Err(storage("read package file", &path, &error)),
        };
        if !present {
            return Err(refuse(format!(
                "{} is not a package directory (it needs {PACKAGE_FILE} and {PACKAGE_WORLD_FILE})",
                dir.display()
            )));
        }
    }
    let manifest = dir.join(PACKAGE_FILE);
    let text = read_bounded(&manifest, MANIFEST_MAX_BYTES)?;
    let package: Package = serde_json::from_slice(&text)
        .map_err(|error| refuse(format!("{}: {error}", manifest.display())))?;
    if package.schema != PACKAGE_SCHEMA {
        return Err(refuse(format!(
            "the manifest's schema is {:?}, not {PACKAGE_SCHEMA}",
            package.schema
        )));
    }
    if package.boundary.exterior != HOST_EXTERIOR {
        return Err(refuse(format!(
            "the package declares exterior {:?}; only {HOST_EXTERIOR:?} is known",
            package.boundary.exterior
        )));
    }
    if package.world.path != PACKAGE_WORLD_FILE {
        return Err(refuse(format!(
            "the container is {:?}, not {PACKAGE_WORLD_FILE}",
            package.world.path
        )));
    }
    let path = dir.join(PACKAGE_WORLD_FILE);
    let bytes = read_bounded(&path, package.world.bytes.min(WORLD_MAX_BYTES))?;
    let sha256 = Sha256Digest::of(&bytes).to_hex();
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != package.world.bytes
        || sha256 != package.world.sha256
    {
        return Err(refuse(format!(
            "the container holds {} bytes with SHA-256 {sha256}; the manifest names {} bytes with {}",
            bytes.len(),
            package.world.bytes,
            package.world.sha256
        )));
    }
    let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS)?;
    let world = World::read_checkpoint(&reader, profile)?;
    drop(bytes);
    let holdings = world.ledger().holdings();
    let disk = package.crop.disk();
    let measured = world.measure_region(disk.center, disk.radius)?;
    let grid = world.field().grid();
    let checks = [
        (
            "crop",
            agrees(package.crop.center[0], disk.center[0])
                && agrees(package.crop.center[1], disk.center[1])
                && agrees(package.crop.radius, disk.radius)
                && measured.cells == package.crop.cells
                && measured.organisms == package.organisms
                && measured.lineages == package.lineages
                && package.crop.grid == [grid.cells_x(), grid.cells_y()]
                && agrees(package.crop.cell_size[0], grid.cell_width())
                && agrees(package.crop.cell_size[1], grid.cell_height()),
        ),
        ("digest", world.digest().to_hex() == package.world.digest),
        ("tick", world.tick().get() == package.source.tick),
        (
            "configuration",
            world.config().config_hash().to_hex() == package.source.config_sha256,
        ),
        (
            "organisms",
            u64::try_from(world.population().len()).ok() == Some(package.organisms),
        ),
        (
            "endowment",
            agrees(package.endowment.field, holdings.field())
                && agrees(package.endowment.organisms, holdings.organisms())
                && agrees(package.endowment.total, holdings.total()),
        ),
    ];
    if let Some((name, _)) = checks.iter().find(|(_, agreed)| !agreed) {
        return Err(refuse(format!(
            "the manifest's {name} disagrees with the restored world"
        )));
    }
    Ok((package, world))
}
