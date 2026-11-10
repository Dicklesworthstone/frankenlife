//! `flife render`: pre-rendered output. The world runs headlessly and every
//! `every` ticks its render epoch is drawn through the same FrankenManim
//! view the Studio shows, on FrankenManim's CPU reference 3D route, and
//! written as a PNG. With `video`, the frames are then encoded by an
//! external `ffmpeg` (plan §3.4: video uses an external encoder boundary).

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Instant;

use flife_backend_metal::ControllerChoice;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::Stepped;
use flife_manim::{Style, WorldView, hex_rgb};
use flife_render::Publisher;
use fmn_codec::CompressionLevel;
use serde::Serialize;

use crate::found;
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

/// One `flife render` record.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "record")]
pub enum RenderRecord {
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
        /// What the controller backend did. Not scientific state.
        controllers: ControllerSummary,
        /// Wall seconds for the whole run.
        wall_seconds: f64,
    },
}

/// Render `config` per `options`, passing each record to `emit`.
///
/// # Errors
/// The world's founding and stepping errors; [`CoreError::StateConflict`]
/// for a capacity pause, an unwritable frame or a failed encoder; and the
/// view's errors.
pub fn render(
    config: &ValidatedConfig,
    options: &RenderOptions,
    mut emit: impl FnMut(&RenderRecord),
) -> Result<(), CoreError> {
    let clock = Instant::now();
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
    // The observer's term (FL-043): the feed's epoch slots as reserved, and
    // the frames in flight, each held raw and encoded. The view's own
    // drawing buffers are not counted.
    let [width, height] = options.size;
    let frame = u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(4);
    let observer = publisher
        .reserved_bytes()
        .saturating_add(frame.saturating_mul(2 * (ENCODERS as u64 + 1)));
    let memory = crate::memory::admit(config, &world, options.controllers, None, observer)?;
    install(&mut world, options.controllers, memory.backend)?;
    std::fs::create_dir_all(&options.out)
        .map_err(|error| io("create the frame directory", &options.out, &error))?;
    let viewer = publisher.feed().viewer(1)?;
    let founding = viewer.latest()?;
    let mut view = WorldView::for_epoch(&founding, options.size, background)?;
    view.set_style(options.style);
    drop(founding);
    let every = options.every.max(1);
    let mut frames = 0;
    // PNG encoding is the largest part of a frame and independent of every
    // other frame, so up to `ENCODERS` frames encode on their own threads
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
                if encoding.len() >= ENCODERS {
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
        controllers: ControllerSummary::of(&world),
        wall_seconds: clock.elapsed().as_secs_f64(),
    });
    Ok(())
}

/// Frames encoding at once: enough to keep pace with rendering, and a
/// bound on the frames held in memory.
const ENCODERS: usize = 4;

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
