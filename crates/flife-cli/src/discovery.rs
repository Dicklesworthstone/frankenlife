//! Saved discoveries (FL-042; plan §14.5, §16.7).
//!
//! "A discovery contains a run/checkpoint reference, selected region or
//! lineage, a thumbnail, its presentation settings, and a description."
//! [`discover`] saves one from a run directory's durable checkpoint into a
//! new (or empty) bundle directory:
//!
//! | Path | Content |
//! |---|---|
//! | `thumbnail.png` | the checkpoint's world drawn through the same FrankenManim view as `flife render`, framed on the region when one is selected, with the region's measurement outline |
//! | `package/` | for a region, the local transplant package ([`crate::package`]): the cropped field and organisms, the declared boundary condition and the energy endowment |
//! | `world/` | the full-world export: a complete, reproducible world checkpoint, as a run directory that `flife resume` continues |
//! | `discovery.json` | the [`Discovery`]: identity, source reference, selection, presentation, description, thumbnail and exports; written last |
//!
//! - **Full-world export.** `world/` is a control branch ([`crate::branch()`])
//!   of the source run at the discovery's checkpoint, named after the
//!   discovery: its own session, inheriting the source's commands, its
//!   founding checkpoint byte-identical to the source's in every world
//!   chunk and linking the source store's verified chunk files. Resumed, it
//!   reaches exactly the state the source run reaches from that checkpoint.
//! - **Local package.** `package/` holds only the region: the organisms in
//!   it and the field cells whose centres lie in it, with the declared
//!   boundary condition `exterior = "host"` (nothing outside the crop
//!   travels with it) and its energy endowment. It is not a run; a
//!   transplant grafts it into a host. A lineage or whole-world discovery
//!   has no disk, so it writes none.
//! - **Pin.** The source store pins the checkpoint for the discovery
//!   ([`flife_store::ChunkStore::pin`]): while the source configuration's
//!   `preserve_pinned_discoveries` holds, its retention keeps the
//!   checkpoint and every chunk it names however old it grows (§16.7). The
//!   pin is made under the same hold on the store as the checkpoint's
//!   choice, after the thumbnail is drawn and before anything is written.
//! - **Commit point.** `discovery.json` is written to a temporary name and
//!   renamed into place after everything else. A bundle without it is
//!   incomplete: a failure after the pin (a write the host refuses, or the
//!   package's or the full-world export's refusals) leaves the pin, which
//!   names the discovery's identity, and whatever of the bundle was
//!   written.
//!
//! The source's store is opened as its writer, so a run another process
//! holds (a running `flife run` or Studio) is refused, as for a branch.

use std::fs::{self, File};
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::Path;
use std::str::FromStr;

use flife_backend_metal::ControllerChoice;
use flife_core::canonical::Sha256Digest;
use flife_core::memory::total;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{NumericalProfile, RegionMeasurement, World};
use flife_manim::{Region, RegionEffect, RegionShape, Style, Viewport, WorldView, hex_rgb};
use flife_render::Epoch;
use flife_store::ChunkStore;
use fmn_codec::CompressionLevel;
use serde::Serialize;

use crate::branch::{BRANCH_NAME_MAX_BYTES, BranchOptions, branch};
use crate::memory::{MemoryAdmission, admit};
use crate::package::{PACKAGE_DIR, Package, PackageSource, write_package};
use crate::persist::{STORE_DIR, StoreDurability, read_config, restorable, run_identity};
use crate::render::{RenderProjection, encoding_bytes};
use crate::run::Record;

/// The schema of a discovery's `discovery.json` and of `flife discover`'s
/// record.
pub const DISCOVERY_SCHEMA: &str = "frankenlife.discovery.v1";

/// The bundle's discovery file.
pub const DISCOVERY_FILE: &str = "discovery.json";

/// The bundle's thumbnail.
pub const THUMBNAIL_FILE: &str = "thumbnail.png";

/// The bundle's full-world export, a run directory.
pub const WORLD_DIR: &str = "world";

/// The longest description.
pub const DESCRIPTION_MAX_BYTES: usize = 4096;

/// The largest thumbnail side, in pixels.
pub const THUMBNAIL_MAX_PIXELS: u32 = 4096;

/// The share of the thumbnail's shorter side a selected region's diameter
/// fills, when the viewport's zoom range allows it.
pub const REGION_FILL: f64 = 0.75;

/// A disk on the world's torus, written `X,Y,RADIUS` in world units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Disk {
    /// Its centre: a world position.
    pub center: [f64; 2],
    /// Its radius in world units.
    pub radius: f64,
}

impl FromStr for Disk {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let parts: Vec<&str> = text.split(',').collect();
        let [x, y, radius] = parts.as_slice() else {
            return Err(format!("`{text}` is not X,Y,RADIUS"));
        };
        let number = |part: &str| {
            part.trim()
                .parse::<f64>()
                .map_err(|error| format!("`{part}` in `{text}`: {error}"))
        };
        Ok(Self {
            center: [number(x)?, number(y)?],
            radius: number(radius)?,
        })
    }
}

/// What a discovery selects in its world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Selection {
    /// The whole world.
    World,
    /// The organisms and field cells in a disk (plan §14.3's measurement
    /// region).
    Region(Disk),
    /// The live organisms of one founder lineage.
    Lineage(u64),
}

/// What to save.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoverOptions {
    /// The committed tick of the source checkpoint; the newest that
    /// restores if `None`.
    pub at: Option<u64>,
    /// The discovery's name, part of its identity; the bundle directory's
    /// name if `None`.
    pub name: Option<String>,
    /// What the discovery is, in the discoverer's words.
    pub description: String,
    /// What it selects.
    pub selection: Selection,
    /// The thumbnail's size in pixels.
    pub size: [u32; 2],
    /// How the thumbnail draws the world.
    pub style: Style,
    /// Threads for FrankenManim's CPU renderer.
    pub render_threads: NonZeroUsize,
    /// Whether to write the full-world export.
    pub full_world: bool,
    /// Whether to write the local package, for a region selection.
    pub package: bool,
    /// How the bundle's files, the export's founding checkpoint and the
    /// pin are made durable.
    pub durability: StoreDurability,
}

/// The run and checkpoint a discovery refers to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Source {
    /// The run directory, as given.
    pub run_dir: String,
    /// Its session.
    pub session: u64,
    /// The checkpoint's sequence number in its store.
    pub sequence: u64,
    /// Its committed tick.
    pub tick: u64,
    /// Its container's SHA-256.
    pub checkpoint_sha256: String,
    /// The restored world's committed digest.
    pub world_digest: String,
    /// The run configuration's hash.
    pub config_sha256: String,
    /// Whether the source store pins the checkpoint for this discovery
    /// (always, once the discovery is saved).
    pub pinned: bool,
    /// `[persistence] preserve_pinned_discoveries`.
    pub pins_preserved: bool,
}

/// A selection, as `discovery.json` records it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Selected {
    /// The whole world.
    World {
        /// Organisms alive.
        organisms: u64,
    },
    /// A disk, measured in the committed world
    /// ([`World::measure_region`]).
    Region {
        /// Its centre.
        center: [f64; 2],
        /// Its radius.
        radius: f64,
        /// Live organisms in it.
        organisms: u64,
        /// Their committed energies, summed in identity order.
        organism_energy: f64,
        /// Distinct founder lineages among them.
        lineages: u64,
        /// Field cells whose centres lie in it.
        cells: u64,
        /// Those cells' committed energies, summed in cell order.
        field_energy: f64,
    },
    /// A founder lineage.
    Lineage {
        /// The lineage's identity.
        lineage: u64,
        /// Its live organisms.
        organisms: u64,
        /// The lowest and highest generation among them.
        generations: [u32; 2],
    },
}

/// The camera the thumbnail was drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Camera {
    /// The world position at the drawable's centre.
    pub center: [f64; 2],
    /// The zoom: 1 fits the whole world.
    pub zoom: f64,
}

/// The presentation settings the thumbnail was drawn with: what a viewer
/// needs to draw the discovery the same way.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Presentation {
    /// The organisms' colour mapping.
    pub mapping: &'static str,
    /// What that colour means.
    pub meaning: &'static str,
    /// `glow` or `raw`.
    pub look: &'static str,
    /// `points` or `density`.
    pub particles: &'static str,
    /// Whether the field is drawn.
    pub field: bool,
    /// What the field's colour means.
    pub field_mapping: &'static str,
    /// The fixed projection the affinity and composition colours take.
    pub projection: RenderProjection,
    /// The camera.
    pub camera: Camera,
    /// The configuration's background, `#rrggbb`.
    pub background: String,
    /// The bloom strength, if bloom is on.
    pub bloom: Option<f64>,
    /// The dots' smallest on-screen radius, in points.
    pub point_radius_pixels: f64,
}

/// The bundle's thumbnail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Thumbnail {
    /// Its path in the bundle.
    pub path: &'static str,
    /// Its width in pixels.
    pub width: u32,
    /// Its height in pixels.
    pub height: u32,
    /// The PNG file's SHA-256.
    pub sha256: String,
}

/// The full-world export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FullWorld {
    /// Its path in the bundle: a run directory.
    pub path: &'static str,
    /// Its session ([`crate::branch_session`] of the checkpoint and the
    /// discovery's name).
    pub session: u64,
    /// Its founding checkpoint's container SHA-256.
    pub checkpoint_sha256: String,
    /// Founding chunks written, and linked from the source's store.
    pub chunks: [usize; 2],
}

/// The local package export.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LocalPackage {
    /// Its path in the bundle: a package directory.
    pub path: &'static str,
    /// Its manifest, `package.json`.
    pub manifest: Package,
}

/// The bundle's exports (plan §14.5: "Support two explicit exports").
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Exports {
    /// "a complete reproducible world checkpoint", if written.
    pub full_world: Option<FullWorld>,
    /// "a local transplant package with declared cropped field, boundary
    /// conditions, and energy endowment", if written: for a region only.
    pub local_package: Option<LocalPackage>,
}

/// A saved discovery: `discovery.json`, and `flife discover`'s record.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Discovery {
    /// [`DISCOVERY_SCHEMA`].
    pub schema: &'static str,
    /// Its identity ([`discovery_id`]), which pins the source checkpoint.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its description.
    pub description: String,
    /// The run and checkpoint it refers to.
    pub source: Source,
    /// What it selects.
    pub selection: Selected,
    /// How the thumbnail was drawn.
    pub presentation: Presentation,
    /// The thumbnail.
    pub thumbnail: Thumbnail,
    /// The exports.
    pub exports: Exports,
    /// The thumbnail render's peak-memory admission (FL-043).
    pub memory: MemoryAdmission,
}

/// A discovery's identity: the SHA-256 of `frankenlife.discovery.v1`, a
/// zero byte, its checkpoint's container SHA-256 and its name.
#[must_use]
pub fn discovery_id(checkpoint: Sha256Digest, name: &str) -> Sha256Digest {
    let mut input = format!("{DISCOVERY_SCHEMA}\0").into_bytes();
    input.extend_from_slice(checkpoint.as_bytes());
    input.extend_from_slice(name.as_bytes());
    Sha256Digest::of(&input)
}

fn conflict(detail: String) -> CoreError {
    CoreError::StateConflict {
        operation: "save discovery",
        detail,
    }
}

pub(crate) fn storage(operation: &'static str, path: &Path, error: &std::io::Error) -> CoreError {
    CoreError::StorageFailed {
        operation,
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}

/// Refuse a bundle directory that holds anything.
fn vacant_bundle(out: &Path) -> Result<(), CoreError> {
    match fs::read_dir(out) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                Err(conflict(format!(
                    "{} is not empty: a discovery goes into a new directory",
                    out.display()
                )))
            } else {
                Ok(())
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(storage("read bundle directory", out, &error)),
    }
}

/// Save a discovery of the run in `run_dir` into the new (or empty)
/// bundle directory `out` (see the [module documentation](self)).
///
/// # Errors
/// [`CoreError::StateConflict`] for a name that is empty or longer than
/// [`BRANCH_NAME_MAX_BYTES`], a description longer than
/// [`DESCRIPTION_MAX_BYTES`], a thumbnail side of zero or above
/// [`THUMBNAIL_MAX_PIXELS`], a bundle directory that is not empty, a run
/// that holds no checkpoint that restores (at `options.at`, if given) or is
/// held by another process, or a lineage with no live organism at the
/// checkpoint; [`CoreError::NumericalInvalid`] for a region a measurement
/// refuses; [`CoreError::BudgetExhausted`] when the thumbnail does not fit
/// the configuration's memory budget; the store's, branch's and view's
/// errors. All but the bundle's writes (the thumbnail, the package and the
/// full-world export, whose errors include the branch's) come before the
/// pin and before the bundle directory is made.
pub fn discover(
    run_dir: &Path,
    out: &Path,
    options: &DiscoverOptions,
) -> Result<Discovery, CoreError> {
    let name = match &options.name {
        Some(name) => name.clone(),
        None => out
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    if name.is_empty() || name.len() > BRANCH_NAME_MAX_BYTES {
        return Err(conflict(format!(
            "a discovery name is 1 to {BRANCH_NAME_MAX_BYTES} bytes, not {}",
            name.len()
        )));
    }
    if options.description.len() > DESCRIPTION_MAX_BYTES {
        return Err(conflict(format!(
            "a description is at most {DESCRIPTION_MAX_BYTES} bytes, not {}",
            options.description.len()
        )));
    }
    if options
        .size
        .iter()
        .any(|side| *side == 0 || *side > THUMBNAIL_MAX_PIXELS)
    {
        return Err(conflict(format!(
            "a thumbnail side is 1 to {THUMBNAIL_MAX_PIXELS} pixels, not {:?}",
            options.size
        )));
    }
    vacant_bundle(out)?;

    let config = read_config(run_dir)?;
    let revision = config.config_hash();
    let (session, _) = run_identity(run_dir, revision)?;
    let profile = NumericalProfile::reference_f32()?;
    // The store stays open, its writer's lock held, from the choice of the
    // checkpoint to its pin, so no other process's retention can take the
    // checkpoint between the two.
    let mut store = ChunkStore::open(run_dir.join(STORE_DIR), options.durability)?;
    let recovered = store.latest_valid(|_, bytes| {
        let restored = restorable(bytes, profile, revision, session)?;
        let tick = restored.world.tick().get();
        if options.at.is_some_and(|at| at != tick) {
            return Err(conflict(format!("it holds tick {tick}")));
        }
        Ok((restored.world, Sha256Digest::of(bytes)))
    })?;
    let Some((sequence, (world, checkpoint))) = recovered.latest else {
        return Err(conflict(format!(
            "{} holds no checkpoint that restores{}",
            run_dir.display(),
            options
                .at
                .map_or_else(String::new, |at| format!(" at tick {at}"))
        )));
    };
    let tick = world.tick().get();
    let epoch = Epoch::standalone(&world)?;
    let selection = selected(&world, &epoch, options.selection)?;
    let id = discovery_id(checkpoint, &name);

    let (presentation, thumbnail, memory, png) = draw(&config, &world, &epoch, options)?;
    drop(epoch);
    // Every refusal above, the render's admission among them, leaves no
    // pin and no bundle directory behind.
    let pins_preserved = config.config().persistence.preserve_pinned_discoveries;
    store.pin(sequence, &id)?;
    // The full-world export opens the store itself.
    drop(store);
    fs::create_dir_all(out).map_err(|error| storage("create bundle directory", out, &error))?;
    write_new(&out.join(THUMBNAIL_FILE), &png, options.durability)?;

    let local_package = match options.selection {
        Selection::Region(disk) if options.package => Some(LocalPackage {
            path: PACKAGE_DIR,
            manifest: write_package(
                &out.join(PACKAGE_DIR),
                &world,
                disk,
                PackageSource {
                    run_dir: run_dir.display().to_string(),
                    session: session.get(),
                    sequence,
                    tick,
                    checkpoint_sha256: checkpoint.to_hex(),
                    config_sha256: revision.to_hex(),
                },
                options.durability,
            )?,
        }),
        Selection::Region(_) | Selection::World | Selection::Lineage(_) => None,
    };
    drop(world);

    let full_world = if options.full_world {
        let record = branch(
            run_dir,
            &out.join(WORLD_DIR),
            &BranchOptions {
                at: Some(tick),
                name: Some(name.clone()),
                commands: Vec::new(),
                durability: options.durability,
            },
        )?;
        let Record::Branch {
            session: exported,
            parent,
            sha256,
            chunks,
            ..
        } = record
        else {
            return Err(conflict("the full-world export made no branch".into()));
        };
        if parent.sha256 != checkpoint.to_hex() || parent.sequence != sequence {
            return Err(conflict(format!(
                "the source's checkpoint at tick {tick} changed while it was saved \
                 (sequence {sequence}, then {})",
                parent.sequence
            )));
        }
        Some(FullWorld {
            path: WORLD_DIR,
            session: exported,
            checkpoint_sha256: sha256,
            chunks: [chunks.written, chunks.linked],
        })
    } else {
        None
    };

    let discovery = Discovery {
        schema: DISCOVERY_SCHEMA,
        id: id.to_hex(),
        name,
        description: options.description.clone(),
        source: Source {
            run_dir: run_dir.display().to_string(),
            session: session.get(),
            sequence,
            tick,
            checkpoint_sha256: checkpoint.to_hex(),
            world_digest: selection.1.to_hex(),
            config_sha256: revision.to_hex(),
            pinned: true,
            pins_preserved,
        },
        selection: selection.0,
        presentation,
        thumbnail,
        exports: Exports {
            full_world,
            local_package,
        },
        memory,
    };
    commit(out, &discovery, options.durability)?;
    Ok(discovery)
}

/// The selection as recorded, and the world's digest.
fn selected(
    world: &World,
    epoch: &Epoch,
    selection: Selection,
) -> Result<(Selected, Sha256Digest), CoreError> {
    let recorded = match selection {
        Selection::World => Selected::World {
            organisms: epoch.len() as u64,
        },
        Selection::Region(disk) => {
            let RegionMeasurement {
                center,
                radius,
                organisms,
                organism_energy,
                lineages,
                cells,
                field_energy,
                ..
            } = world.measure_region(disk.center, disk.radius)?;
            Selected::Region {
                center,
                radius,
                organisms,
                organism_energy,
                lineages,
                cells,
                field_energy,
            }
        }
        Selection::Lineage(lineage) => {
            let generations: Vec<u32> = epoch
                .lineages()
                .iter()
                .zip(epoch.generations())
                .filter(|(found, _)| **found == lineage)
                .map(|(_, generation)| *generation)
                .collect();
            let (Some(lowest), Some(highest)) = (
                generations.iter().min().copied(),
                generations.iter().max().copied(),
            ) else {
                return Err(conflict(format!(
                    "lineage {lineage} has no live organism at tick {}",
                    world.tick().get()
                )));
            };
            Selected::Lineage {
                lineage,
                organisms: generations.len() as u64,
                generations: [lowest, highest],
            }
        }
    };
    Ok((recorded, world.digest()))
}

/// Draw the thumbnail: the presentation, the thumbnail's record, the
/// render's admission and the PNG's bytes. It writes nothing.
fn draw(
    config: &ValidatedConfig,
    world: &World,
    epoch: &Epoch,
    options: &DiscoverOptions,
) -> Result<(Presentation, Thumbnail, MemoryAdmission, Vec<u8>), CoreError> {
    let model = config.config();
    let background =
        hex_rgb(&model.presentation.background).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "presentation background",
            detail: format!("`{}` is not #rrggbb", model.presentation.background),
        })?;
    let mut view = WorldView::for_epoch(epoch, options.size, background)?;
    view.set_style(options.style);
    let bloom = model
        .presentation
        .bloom_enabled
        .then_some(model.presentation.bloom_strength);
    view.set_bloom(bloom)?;
    view.set_point_radius(model.presentation.point_radius_pixels)?;
    let fitted = Viewport::fit(epoch.extent(), options.size)?;
    let camera = match options.selection {
        Selection::Region(disk) => {
            let shorter = f64::from(options.size[0].min(options.size[1]));
            let zoom = (REGION_FILL * shorter / (2.0 * disk.radius * fitted.fit_scale()))
                .clamp(1.0, flife_manim::MAXIMUM_ZOOM);
            let viewport = fitted.with_view(disk.center, zoom)?;
            view.set_view(viewport)?;
            view.set_region(Some(Region {
                shape: RegionShape::Disc {
                    center: disk.center,
                    radius: disk.radius,
                },
                effect: RegionEffect::Measures,
            }))?;
            Camera {
                center: viewport.center(),
                zoom: viewport.zoom(),
            }
        }
        Selection::World | Selection::Lineage(_) => Camera {
            center: fitted.center(),
            zoom: fitted.zoom(),
        },
    };
    let threads = options.render_threads.get();
    let memory = admit(
        world,
        ControllerChoice::Cpu,
        None,
        total([
            epoch.reserved_bytes(),
            view.reserved_bytes(),
            view.frame_bytes(),
            view.offline_bytes(threads),
            encoding_bytes(options.size, 1),
        ]),
    )?;
    view.show(epoch)?;
    let rgba = view.render_rgba8(threads)?;
    let [width, height] = options.size;
    let png = fmn_codec::png::encode_rgba8(width, height, &rgba, CompressionLevel::Default);
    let style = options.style;
    Ok((
        Presentation {
            mapping: style.mapping.name(),
            meaning: style.mapping.meaning(),
            look: style.look.name(),
            particles: style.particles.name(),
            field: style.field,
            field_mapping: style.field_mapping.name(),
            projection: RenderProjection::of(config),
            camera,
            background: model.presentation.background.clone(),
            bloom,
            point_radius_pixels: model.presentation.point_radius_pixels,
        },
        Thumbnail {
            path: THUMBNAIL_FILE,
            width,
            height,
            sha256: Sha256Digest::of(&png).to_hex(),
        },
        memory,
        png,
    ))
}

/// Write `bytes` to the new file `path`, flushed under `durability`.
pub(crate) fn write_new(
    path: &Path,
    bytes: &[u8],
    durability: StoreDurability,
) -> Result<(), CoreError> {
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| storage("create bundle file", path, &error))?;
    file.write_all(bytes)
        .map_err(|error| storage("write bundle file", path, &error))?;
    if durability == StoreDurability::PowerLoss {
        file.sync_all()
            .map_err(|error| storage("flush bundle file", path, &error))?;
    }
    Ok(())
}

/// Write `discovery.json`: to a temporary name, then renamed into place,
/// the bundle's commit point.
fn commit(out: &Path, discovery: &Discovery, durability: StoreDurability) -> Result<(), CoreError> {
    let mut text =
        serde_json::to_string_pretty(discovery).map_err(|error| CoreError::StateConflict {
            operation: "encode discovery",
            detail: error.to_string(),
        })?;
    text.push('\n');
    publish(out, DISCOVERY_FILE, text.as_bytes(), durability)
}

/// Write `bytes` as the new file `name` in `dir` all at once: to
/// `name.partial`, renamed into place, and under `power-loss` the file and
/// then the directory flushed. A reader finds the whole file or none.
pub(crate) fn publish(
    dir: &Path,
    name: &str,
    bytes: &[u8],
    durability: StoreDurability,
) -> Result<(), CoreError> {
    let partial = dir.join(format!("{name}.partial"));
    write_new(&partial, bytes, durability)?;
    let path = dir.join(name);
    fs::rename(&partial, &path).map_err(|error| storage("publish file", &path, &error))?;
    if durability == StoreDurability::PowerLoss {
        File::open(dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| storage("flush directory", dir, &error))?;
    }
    Ok(())
}
