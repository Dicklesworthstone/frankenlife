//! `flife render`: pre-rendered output. The world runs headlessly and every
//! `every` ticks its render epoch is drawn through the same FrankenManim
//! view the Studio shows, on FrankenManim's CPU reference 3D route, and
//! written as a PNG. With `video`, the frames are then encoded by an
//! external `ffmpeg` (plan §3.4: video uses an external encoder boundary).

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Instant;

use flife_backend_metal::ControllerChoice;
use flife_core::memory::total;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Stepped, World};
use flife_manim::{Style, WorldView, hex_rgb};
use flife_render::{AffinityProjection, Publisher};
use fmn_codec::CompressionLevel;
use serde::Serialize;

use crate::found;
use crate::memory::MemoryAdmission;
use crate::run::{ControllerSummary, install};

/// The schema of `flife render`'s NDJSON records.
pub const RENDER_SCHEMA: &str = "frankenlife.render.v1";

/// What to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderOptions {
    /// Ticks to run after the founding frame.
    pub ticks: u64,
    /// Draw a frame every this many ticks (at least one).
    pub every: u64,
    /// The frame size in pixels.
    pub size: [u32; 2],
    /// The directory the PNG frames go to; created if missing.
    pub out: PathBuf,
    /// Threads for the world, replacing `[execution] cpu_workers`.
    pub cpu_workers: Option<NonZeroUsize>,
    /// Where due controllers are evaluated. No result depends on it.
    pub controllers: ControllerChoice,
    /// Threads FrankenManim's CPU renderer may use.
    pub render_threads: NonZeroUsize,
    /// An MP4 to encode the frames into with `ffmpeg`, at `fps`.
    pub video: Option<PathBuf>,
    /// The video's frames per second.
    pub fps: u32,
    /// How the frames are drawn: colour mapping, look, points or density,
    /// and the field.
    pub style: Style,
}

/// Plan §13.4: "A fixed high-dimensional projection has a recorded matrix
/// and scaling." The projection the frames' affinity and composition
/// colours are drawn through ([`AffinityProjection`]): display axis `k` of a
/// 32-channel vector is the sum of `matrix[k]` times it over `divisor`
/// times its unit, `affinity_limit` for an organism's affinity and a
/// block's energy for the field's composition.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RenderProjection {
    /// The rows of the 32 × 32 Sylvester Hadamard matrix taken.
    pub rows: [usize; 3],
    /// The matrix, a row per display axis and a column per channel.
    pub matrix: [[i8; 32]; 3],
    /// `√32`.
    pub divisor: f64,
    /// `chemistry.affinity_norm_limit`.
    pub affinity_limit: f64,
}

impl RenderProjection {
    /// [`AffinityProjection::fixed`] for `config`'s world.
    #[must_use]
    pub fn of(config: &ValidatedConfig) -> Self {
        let projection = AffinityProjection::fixed();
        Self {
            rows: projection.rows,
            matrix: projection.matrix,
            divisor: projection.divisor,
            affinity_limit: config.config().chemistry.affinity_norm_limit,
        }
    }
}

/// One `flife render` record.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "record")]
pub enum RenderRecord {
    /// The run was admitted (FL-043): emitted once, before any frame.
    Start {
        /// [`RENDER_SCHEMA`].
        schema: &'static str,
        /// The peak-memory admission, its observers' term included.
        memory: MemoryAdmission,
        /// Frames encoded at once: [`ENCODERS`], or fewer when the budget
        /// is short.
        encoders: usize,
    },
    /// A frame written.
    Frame {
        /// [`RENDER_SCHEMA`].
        schema: &'static str,
        /// The frame's index.
        frame: u64,
        /// The world tick it shows.
        tick: u64,
        /// Organisms alive.
        live: usize,
        /// The live organisms' mean speed, world units per simulated second.
        mean_speed: f64,
        /// The fastest live organism's speed.
        top_speed: f64,
        /// The PNG.
        path: String,
        /// Seconds rendering and encoding it took.
        render_seconds: f64,
    },
    /// The run ended.
    End {
        /// [`RENDER_SCHEMA`].
        schema: &'static str,
        /// Frames written.
        frames: u64,
        /// The last tick.
        tick: u64,
        /// The committed world's digest.
        digest: String,
        /// The encoded video, if any.
        video: Option<String>,
        /// The colour mapping the frames show.
        mapping: &'static str,
        /// What that colour means.
        meaning: &'static str,
        /// `glow` or `raw`.
        look: &'static str,
        /// `points` or `density`.
        particles: &'static str,
        /// Whether the field is drawn.
        field: bool,
        /// What the field's colour means: `energy` or `composition`.
        field_mapping: &'static str,
        /// The fixed projection the affinity and composition colours take
        /// (boxed: its matrix is most of the record).
        projection: Box<RenderProjection>,
        /// What the controller backend did. Not scientific state.
        controllers: ControllerSummary,
        /// Wall seconds for the whole run.
        wall_seconds: f64,
    },
}

/// Render `config` per `options`, passing each record to `emit`.
///
/// # Errors
/// [`video_encoder`]'s, before anything is founded or written, when a video
/// is asked for. The world's founding and stepping errors;
/// [`CoreError::StateConflict`] for a capacity pause, an unwritable frame
/// or a failed encoder; and the view's errors.
pub fn render(
    config: &ValidatedConfig,
    options: &RenderOptions,
    mut emit: impl FnMut(&RenderRecord),
) -> Result<(), CoreError> {
    let clock = Instant::now();
    // A video's encoder runs after every frame is drawn, so it is checked
    // before the first.
    if options.video.is_some() {
        video_encoder()?;
    }
    let model = config.config();
    let background =
        hex_rgb(&model.presentation.background).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "presentation background",
            detail: format!("`{}` is not #rrggbb", model.presentation.background),
        })?;
    let mut world = found(config)?;
    if let Some(workers) = options.cpu_workers {
        world.set_cpu_workers(workers)?;
    }
    let mut publisher = Publisher::new(&world, 3, || {})?;
    let viewer = publisher.feed().viewer(1)?;
    let founding = viewer.latest()?;
    let mut view = WorldView::for_epoch(&founding, options.size, background)?;
    view.set_style(options.style);
    let presentation = &model.presentation;
    view.set_bloom(
        presentation
            .bloom_enabled
            .then_some(presentation.bloom_strength),
    )?;
    view.set_point_radius(presentation.point_radius_pixels)?;
    drop(founding);
    let (memory, encoders) = admit(&world, options, &publisher, &view)?;
    install(&mut world, options.controllers, memory.backend)?;
    emit(&RenderRecord::Start {
        schema: RENDER_SCHEMA,
        memory,
        encoders,
    });
    std::fs::create_dir_all(&options.out)
        .map_err(|error| io("create the frame directory", &options.out, &error))?;
    let every = options.every.max(1);
    let mut frames = 0;
    // PNG encoding is the largest part of a frame and independent of every
    // other frame, so up to `encoders` frames encode on their own threads
    // while the world steps and the next frame renders. The bytes written
    // are the same; only when they are written changes.
    std::thread::scope(|scope| -> Result<(), CoreError> {
        let mut encoding = std::collections::VecDeque::new();
        for tick in 0..=options.ticks {
            if tick % every == 0 {
                publisher.offer(&world)?;
                let lease = viewer.latest()?;
                let started = Instant::now();
                view.show(&lease)?;
                let live = lease.len();
                let (mean_speed, top_speed) = speeds(lease.velocities());
                drop(lease);
                let rgba = view.render_rgba8(options.render_threads.get())?;
                let path = options.out.join(format!("frame-{frames:06}.png"));
                if encoding.len() >= encoders {
                    finish(encoding.pop_front())?;
                }
                let [width, height] = options.size;
                let target = path.clone();
                encoding.push_back(scope.spawn(move || {
                    let png =
                        fmn_codec::png::encode_rgba8(width, height, &rgba, CompressionLevel::Fast);
                    std::fs::write(&target, png)
                        .map_err(|error| io("write a frame", &target, &error))
                }));
                emit(&RenderRecord::Frame {
                    schema: RENDER_SCHEMA,
                    frame: frames,
                    tick: world.tick().get(),
                    live,
                    mean_speed,
                    top_speed,
                    path: path.display().to_string(),
                    render_seconds: started.elapsed().as_secs_f64(),
                });
                frames += 1;
            }
            if tick < options.ticks {
                match world.step()? {
                    Stepped::Published(_) => {}
                    Stepped::Paused(shortfall) => {
                        return Err(CoreError::StateConflict {
                            operation: "render",
                            detail: format!(
                                "the world paused at tick {}: {shortfall:?}",
                                world.tick().get()
                            ),
                        });
                    }
                }
            }
        }
        while let Some(handle) = encoding.pop_front() {
            finish(Some(handle))?;
        }
        Ok(())
    })?;
    let video = match &options.video {
        Some(video) => {
            encode(&options.out, video, options.fps)?;
            Some(video.display().to_string())
        }
        None => None,
    };
    emit(&RenderRecord::End {
        schema: RENDER_SCHEMA,
        frames,
        tick: world.tick().get(),
        digest: world.digest().to_hex(),
        video,
        mapping: options.style.mapping.name(),
        meaning: options.style.mapping.meaning(),
        look: options.style.look.name(),
        particles: options.style.particles.name(),
        field: options.style.field,
        field_mapping: options.style.field_mapping.name(),
        projection: Box::new(RenderProjection::of(config)),
        controllers: ControllerSummary::of(&world),
        wall_seconds: clock.elapsed().as_secs_f64(),
    });
    Ok(())
}

/// The most frames encoding at once: enough to keep pace with rendering,
/// and a bound on the frames held in memory.
pub const ENCODERS: usize = 4;

/// The most one frame being encoded holds per pixel (FL-043): its RGBA8
/// pixels, 4 bytes, and the PNG encoder's working set, which holds the
/// filtered rows, the compressor's copy of them, its match chains and
/// tokens, and the stream and file it writes. Measured on the pinned
/// FrankenManim encoder: at most 44 bytes a pixel on incompressible noise
/// from 640 x 360 to 1920 x 1080, and 24 on a flat frame.
pub const ENCODING_BYTES_PER_PIXEL: u64 = 4 + 48;

/// What one frame being encoded holds whatever its size: the compressor's
/// hash table and its thread. Measured at under 1 MiB.
pub const ENCODING_BYTES_FIXED: u64 = 2 << 20;

/// What `encoders` frames being encoded at once hold at `size`.
#[must_use]
pub fn encoding_bytes(size: [u32; 2], encoders: usize) -> u64 {
    let [width, height] = size.map(u64::from);
    width
        .saturating_mul(height)
        .saturating_mul(ENCODING_BYTES_PER_PIXEL)
        .saturating_add(ENCODING_BYTES_FIXED)
        .saturating_mul(encoders as u64)
}

/// Admit `world`'s render (FL-043). The observers' term is the feed's epoch
/// slots as reserved; the view's own buffers, one frame's job and the CPU
/// route's frame buffers ([`WorldView::reserved_bytes`],
/// [`WorldView::frame_bytes`], [`WorldView::offline_bytes`]); and the
/// frames being encoded ([`encoding_bytes`]). [`ENCODERS`] frames encode at
/// once, or as many as the budget leaves room for, one at least, so a
/// small budget slows a render before it refuses one. The backend gets
/// what remains.
fn admit(
    world: &World,
    options: &RenderOptions,
    publisher: &Publisher,
    view: &WorldView,
) -> Result<(MemoryAdmission, usize), CoreError> {
    let drawing = total([
        publisher.reserved_bytes(),
        view.reserved_bytes(),
        view.frame_bytes(),
        view.offline_bytes(options.render_threads.get()),
    ]);
    let observers = |encoders| drawing.saturating_add(encoding_bytes(options.size, encoders));
    for encoders in (2..=ENCODERS).rev() {
        if let Ok(memory) =
            crate::memory::admit(world, options.controllers, None, observers(encoders))
        {
            return Ok((memory, encoders));
        }
    }
    crate::memory::admit(world, options.controllers, None, observers(1)).map(|memory| (memory, 1))
}

/// Wait for one frame's encoder and pass on its result.
fn finish(
    handle: Option<std::thread::ScopedJoinHandle<'_, Result<(), CoreError>>>,
) -> Result<(), CoreError> {
    match handle {
        Some(handle) => handle.join().unwrap_or_else(|_| {
            Err(CoreError::StateConflict {
                operation: "encode a frame",
                detail: "a frame encoder panicked".to_owned(),
            })
        }),
        None => Ok(()),
    }
}

/// The mean and largest of `velocities`' magnitudes; zeros when empty.
fn speeds(velocities: &[[f32; 2]]) -> (f64, f64) {
    let (sum, top) = velocities
        .iter()
        .fold((0.0, 0.0_f64), |(sum, top), [x, y]| {
            let speed = f64::from(*x).hypot(f64::from(*y));
            (sum + speed, top.max(speed))
        });
    let mean = if velocities.is_empty() {
        0.0
    } else {
        sum / velocities.len() as f64
    };
    (mean, top)
}

/// The external encoder `flife render --video` runs: `ffmpeg` on the
/// `PATH`, listing a `libx264` encoder. Returns ffmpeg's version line.
///
/// # Errors
/// [`CoreError::MissingCapability`] when ffmpeg cannot be run, exits with
/// an error, or lists no libx264 encoder.
pub fn video_encoder() -> Result<String, CoreError> {
    let ffmpeg = |args: &[&str]| -> Result<String, CoreError> {
        let output = std::process::Command::new("ffmpeg")
            .args(args)
            .output()
            .map_err(|error| CoreError::MissingCapability {
                capability: format!("ffmpeg, to encode a video: {error}"),
            })?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(CoreError::MissingCapability {
                capability: format!(
                    "ffmpeg, to encode a video: `ffmpeg {}` exited with {}",
                    args.join(" "),
                    output.status
                ),
            })
        }
    };
    let encoders = ffmpeg(&["-hide_banner", "-encoders"])?;
    if !encoders
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some("libx264"))
    {
        return Err(CoreError::MissingCapability {
            capability: "ffmpeg's libx264 encoder, to encode a video: this ffmpeg lists none"
                .to_owned(),
        });
    }
    let version = ffmpeg(&["-version"])?;
    Ok(version.lines().next().unwrap_or_default().trim().to_owned())
}

/// Encode `frames/frame-%06d.png` into an H.264 MP4 with `ffmpeg`.
fn encode(frames: &Path, video: &Path, fps: u32) -> Result<(), CoreError> {
    let output = std::process::Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-framerate"])
        .arg(fps.max(1).to_string())
        .arg("-i")
        .arg(frames.join("frame-%06d.png"))
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "18"])
        .arg(video)
        .output()
        .map_err(|error| CoreError::MissingCapability {
            capability: format!("ffmpeg, to encode a video: {error}"),
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(CoreError::StateConflict {
            operation: "encode video",
            detail: format!(
                "ffmpeg exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        })
    }
}

fn io(operation: &'static str, path: &Path, error: &std::io::Error) -> CoreError {
    CoreError::StateConflict {
        operation,
        detail: format!("{}: {error}", path.display()),
    }
}
