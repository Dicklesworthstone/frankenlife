//! FL-023's basic export (plan §13.8's live capture, G10). [`export`]
//! replays a recorded timeline ([`crate::replay`]) through FrankenManim's
//! frame and encoding paths into a directory of its own:
//!
//! - `frame-NNNNNN.png`: every frame the Studio drew, as the replay draws
//!   it again, in fmn-codec's lossless PNG. A frame sequence is what strict
//!   visual regression compares.
//! - `capture.y4m`: the same frames on a constant clock of the declared
//!   rate, from the first frame's wall time to the last's. Each slot shows
//!   the last frame drawn by its time. The frames are converted by
//!   fmn-frame's `rgba_to_nv12` (BT.709, limited range, left chroma siting)
//!   and written as fmn-codec's y4m (`C420mpeg2`). The manifest declares
//!   how many slots each frame fills, the slots that repeat a frame, and the
//!   frames no slot shows. Encoded video is compared by these settings, and
//!   is not assumed to be byte-identical across encoders.
//! - `capture.mp4`, when asked for ([`ExportOptions::mp4`]): that video
//!   encoded as H.264 by the ffmpeg on the `PATH`
//!   ([`flife_cli::encode_y4m`]), each slot a frame, its samples tagged
//!   with what they hold and compressed, not converted. It plays where y4m
//!   does not. The ffmpeg is checked before anything is written.
//! - `manifest.json`, written last, so a directory without one holds an
//!   export that did not finish. It records the timeline's and the
//!   configuration's hashes, the ticks shown and the tick the session
//!   closed on, the closing world's digest, the renderer, encoders,
//!   resolution, rate and tone mapping, and each file's SHA-256.
//!
//! The directory must not exist. An export never writes over a file.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};

use flife_cli::CameraPath;
use flife_core::model::is_v2_model;
use flife_core::{CoreError, Sha256Digest, Sha256Stream, V2Feature, ValidatedConfig};
use flife_render::AffinityProjection;
use fmn_codec::{CompressionLevel, PngEncoder, Y4mColorspace, append_y4m_frame_nv12, y4m_header};
use fmn_frame::convert::rgba_to_nv12;
use fmn_frame::{ChromaSiting, ColorRange, FrameBuffer, FrameLayout, PixelFormat};
use serde::{Deserialize, Serialize};

use crate::memory::export_scratch_bytes;
use crate::timeline::{
    ReplayConfig, TIMELINE_SCHEMA, TimelineProjection, TimelineRecord, TimelineStart,
    read_timeline, replay_with, validate_timeline,
};

/// Standard Cinema 1080p output resolution (`1920 × 1080`, FL-050).
pub const CINEMA_1080P: [u32; 2] = [1920, 1080];

/// Standard Cinema 4K UHD output resolution (`3840 × 2160`, FL-050).
pub const CINEMA_4K: [u32; 2] = [3840, 2160];

/// Square 4K Cinema output resolution (`2160 × 2160`, FL-050), ideal for
/// square periodic torus worlds with zero letterboxing.
pub const CINEMA_4K_SQUARE: [u32; 2] = [2160, 2160];

/// The schema of an export's manifest.
pub const EXPORT_SCHEMA: &str = "frankenlife.export.v1";

/// The manifest's file name in an export's directory.
pub const MANIFEST_FILE: &str = "manifest.json";

/// The video's file name in an export's directory.
pub const VIDEO_FILE: &str = "capture.y4m";

/// The encoded video's file name in an export's directory.
pub const ENCODED_FILE: &str = "capture.mp4";

/// How a timeline is exported in live-capture mode (FL-023).
#[derive(Debug, Clone, Copy)]
pub struct ExportOptions {
    /// The video's constant rate.
    pub frames_per_second: NonZeroU32,
    /// Threads for the replayed world's field and organism phases; no
    /// result depends on it.
    pub cpu_workers: Option<NonZeroUsize>,
    /// Also encode the video as H.264 into `capture.mp4`, with the ffmpeg
    /// on the `PATH`, which must list libx264.
    pub mp4: bool,
}

/// How frames of a timeline are sampled for export (plan §13.8, FL-050).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExportMode {
    /// Replay every frame the Studio presented at its recorded wall times,
    /// mapped onto a constant video clock (FL-023).
    #[default]
    LiveCapture,
    /// Sample the replayed world at exact, deterministic integer simulation
    /// tick intervals (`every_ticks`), independent of wall-clock recording
    /// jitter (FL-050, G10). Every sampled frame fills exactly 1 video slot
    /// (`repeated = 0`, `dropped = 0`).
    SimulationTime {
        /// Committed simulation ticks between consecutive exported frames.
        every_ticks: NonZeroU64,
    },
}

/// Cinema export options (FL-050, G10): supports both `LiveCapture` and
/// exact `SimulationTime` sampling, optional 1080p/4K target resolution,
/// optional keyframed [`CameraPath`], and full FL-043 / INV-15 peak-memory
/// admission.
#[derive(Debug, Clone)]
pub struct CinemaOptions {
    /// The video's constant rate.
    pub frames_per_second: NonZeroU32,
    /// Threads for the replayed world's field and organism phases; no
    /// result depends on it.
    pub cpu_workers: Option<NonZeroUsize>,
    /// Also encode the video as H.264 into `capture.mp4`, with the ffmpeg
    /// on the `PATH`, which must list libx264.
    pub mp4: bool,
    /// Live-capture or exact simulation-time sampling.
    pub mode: ExportMode,
    /// Optional target output resolution in pixels (e.g., [`CINEMA_1080P`] or
    /// [`CINEMA_4K`]). `None` keeps the timeline's recorded drawables.
    pub resolution: Option<[u32; 2]>,
    /// Optional pixel density (`pixels_per_point`) paired with `resolution`.
    /// When `None`, defaults to `2.0` at 4K (`>= 3840 × 2160`) and `1.0`
    /// otherwise so HUD typography scales cleanly.
    pub pixels_per_point: Option<f64>,
    /// Optional torus-aware smoothstep keyframed camera path evaluated at
    /// each frame's integer committed tick.
    pub camera: Option<CameraPath>,
}

impl From<ExportOptions> for CinemaOptions {
    fn from(options: ExportOptions) -> Self {
        Self {
            frames_per_second: options.frames_per_second,
            cpu_workers: options.cpu_workers,
            mp4: options.mp4,
            mode: ExportMode::LiveCapture,
            resolution: None,
            pixels_per_point: None,
            camera: None,
        }
    }
}

impl CinemaOptions {
    /// Cinema 1080p (`1920 × 1080`, `1.0` pixels/point) simulation-time
    /// options sampling every `every_ticks` committed ticks.
    #[must_use]
    pub const fn cinema_1080p(frames_per_second: NonZeroU32, every_ticks: NonZeroU64) -> Self {
        Self {
            frames_per_second,
            cpu_workers: None,
            mp4: false,
            mode: ExportMode::SimulationTime { every_ticks },
            resolution: Some(CINEMA_1080P),
            pixels_per_point: Some(1.0),
            camera: None,
        }
    }

    /// Cinema 4K UHD (`3840 × 2160`, `2.0` pixels/point) simulation-time
    /// options sampling every `every_ticks` committed ticks.
    #[must_use]
    pub const fn cinema_4k(frames_per_second: NonZeroU32, every_ticks: NonZeroU64) -> Self {
        Self {
            frames_per_second,
            cpu_workers: None,
            mp4: false,
            mode: ExportMode::SimulationTime { every_ticks },
            resolution: Some(CINEMA_4K),
            pixels_per_point: Some(2.0),
            camera: None,
        }
    }

    /// Square 4K Cinema (`2160 × 2160`, `2.0` pixels/point) simulation-time
    /// options sampling every `every_ticks` committed ticks.
    #[must_use]
    pub const fn cinema_4k_square(frames_per_second: NonZeroU32, every_ticks: NonZeroU64) -> Self {
        Self {
            frames_per_second,
            cpu_workers: None,
            mp4: false,
            mode: ExportMode::SimulationTime { every_ticks },
            resolution: Some(CINEMA_4K_SQUARE),
            pixels_per_point: Some(2.0),
            camera: None,
        }
    }

    /// Validate and resolve the optional Cinema `(drawable, pixels_per_point)`
    /// target before touching disk or allocating buffers.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for zero dimensions or non-finite /
    /// non-positive `pixels_per_point`.
    pub fn resolved_target(&self) -> Result<Option<([u32; 2], f64)>, CoreError> {
        if let Some(scale) = self.pixels_per_point
            && (!scale.is_finite() || scale <= 0.0)
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "cinema export pixel density",
                detail: format!("pixels_per_point {scale} must be finite and positive"),
            });
        }
        let Some(size) = self.resolution else {
            return Ok(None);
        };
        if size[0] == 0 || size[1] == 0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "cinema export resolution",
                detail: format!("resolution {size:?} must be non-zero"),
            });
        }
        let scale = self.pixels_per_point.unwrap_or_else(|| {
            if size[0] >= CINEMA_4K[0] || size[1] >= CINEMA_4K[1] {
                2.0
            } else {
                1.0
            }
        });
        Ok(Some((size, scale)))
    }
}

/// Parse a Cinema resolution flag: `1080p`, `4k` / `2160p` (`3840×2160`),
/// `4k-square` / `square-4k` (`2160×2160`), or `WxH` (e.g. `2160x2160`,
/// `3840x2160`).
///
/// # Errors
/// Returns a human-readable explanation if the input is not a positive `WxH`
/// pair or named Cinema preset.
pub fn parse_cinema_resolution(text: &str) -> Result<[u32; 2], String> {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("1080p") {
        return Ok(CINEMA_1080P);
    }
    if trimmed.eq_ignore_ascii_case("4k") || trimmed.eq_ignore_ascii_case("2160p") {
        return Ok(CINEMA_4K);
    }
    if trimmed.eq_ignore_ascii_case("4k-square") || trimmed.eq_ignore_ascii_case("square-4k") {
        return Ok(CINEMA_4K_SQUARE);
    }
    let Some((w, h)) = trimmed.split_once(['x', 'X']) else {
        return Err(format!(
            "`{text}` is not `1080p`, `4k`, `4k-square`, or `WIDTHxHEIGHT`"
        ));
    };
    let width: u32 = w
        .trim()
        .parse()
        .map_err(|_| format!("`{text}` has an invalid width"))?;
    let height: u32 = h
        .trim()
        .parse()
        .map_err(|_| format!("`{text}` has an invalid height"))?;
    if width == 0 || height == 0 {
        return Err(format!("`{text}` must have positive width and height"));
    }
    Ok([width, height])
}

/// One exact integer-tick sample of a Cinema simulation-time export (FL-050).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimulationSample {
    /// Sequential exported frame index (`0, 1, 2, ...`).
    pub frame: u64,
    /// Exact committed simulation tick.
    pub tick: u64,
    /// Exact simulation time in seconds (`tick * seconds_per_tick`).
    pub sim_seconds: f64,
    /// Constant-rate output video timestamp (`frame / fps`).
    pub video_seconds: f64,
}

/// Plan exact integer-tick simulation-time samples over `[start_tick, end_tick]`
/// stepping by `every_ticks` (FL-050, plan §13.8). Uses pure integer tick
/// arithmetic so simulation time never drifts from accumulated floating-point
/// timesteps.
///
/// # Errors
/// [`CoreError::StateConflict`] if `start_tick > end_tick`, `seconds_per_tick`
/// is not finite and positive, or the sample count exceeds `u32::MAX`.
pub fn plan_simulation_samples(
    start_tick: u64,
    end_tick: u64,
    every_ticks: NonZeroU64,
    seconds_per_tick: f64,
    fps: NonZeroU32,
) -> Result<Vec<SimulationSample>, CoreError> {
    if start_tick > end_tick {
        return Err(conflict(format!(
            "simulation-time range [{start_tick}, {end_tick}] is inverted"
        )));
    }
    if !seconds_per_tick.is_finite() || seconds_per_tick <= 0.0 {
        return Err(conflict(format!(
            "timestep {seconds_per_tick} must be finite and positive"
        )));
    }
    let step = every_ticks.get();
    let count = (end_tick - start_tick) / step + 1;
    if count > u64::from(u32::MAX) {
        return Err(conflict(format!(
            "simulation-time export would emit {count} frames, exceeding {}",
            u32::MAX
        )));
    }
    let count_usize = usize::try_from(count).map_err(|_| CoreError::Overflow {
        quantity: "cinema simulation-time sample count",
    })?;
    let rate = f64::from(fps.get());
    let mut samples = Vec::with_capacity(count_usize);
    for frame in 0..count {
        let tick =
            start_tick
                .checked_add(frame.saturating_mul(step))
                .ok_or(CoreError::Overflow {
                    quantity: "cinema simulation-time tick",
                })?;
        #[expect(
            clippy::cast_precision_loss,
            reason = "tick and frame counts far below 2^53 are exact in f64"
        )]
        let (sim_seconds, video_seconds) = (tick as f64 * seconds_per_tick, frame as f64 / rate);
        samples.push(SimulationSample {
            frame,
            tick,
            sim_seconds,
            video_seconds,
        });
    }
    Ok(samples)
}

/// One frame of an export: a frame the Studio drew (or sampled in
/// simulation-time mode).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportedFrame {
    /// The Studio's index of the frame.
    pub frame: u64,
    /// The tick of the epoch it shows.
    pub tick: u64,
    /// When the Studio drew it, in seconds since it opened (or `frame / fps`
    /// in simulation-time export).
    pub wall_seconds: f64,
    /// Exact simulation time in seconds (`tick * seconds_per_tick`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sim_seconds: Option<f64>,
    /// Active organism colour mapping rendered on this frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapping: Option<String>,
    /// Active field mapping rendered on this frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_mapping: Option<String>,
    /// Rendered viewport center in world units (G10 camera alignment).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_center: Option<[f64; 2]>,
    /// Rendered viewport zoom (G10 camera alignment).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_zoom: Option<f64>,
    /// Its PNG's file name.
    pub file: String,
    /// Its PNG's SHA-256.
    pub sha256: String,
    /// The video slots it fills; 0 when no slot shows it.
    pub slots: u64,
}

/// An export's video.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportedVideo {
    /// Its file name.
    pub file: String,
    /// What wrote it.
    pub encoder: String,
    /// Its y4m colour space.
    pub colorspace: String,
    /// The RGB-to-YCbCr matrix.
    pub matrix: String,
    /// The code range.
    pub range: String,
    /// The chroma siting.
    pub chroma_siting: String,
    /// Width and height in pixels.
    pub size: [u32; 2],
    /// Its constant rate.
    pub frames_per_second: u32,
    /// Its frames: the clock's slots.
    pub slots: u64,
    /// The slots that repeat the frame before them.
    pub repeated: u64,
    /// The frames drawn that no slot shows.
    pub dropped: u64,
    /// Its SHA-256.
    pub sha256: String,
}

/// An export's video encoded as H.264 ([`ExportOptions::mp4`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportedEncoding {
    /// Its file name.
    pub file: String,
    /// The video it encodes.
    pub source: String,
    /// The first line of the ffmpeg's `-version`.
    pub ffmpeg: String,
    /// ffmpeg's arguments after the source ([`flife_cli::Y4M_TO_H264`]).
    pub settings: String,
    /// Its SHA-256.
    pub sha256: String,
}

/// An export's manifest ([`EXPORT_SCHEMA`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportManifest {
    /// [`EXPORT_SCHEMA`].
    pub schema: String,
    /// The export mode: `live-capture`, what the Studio presented.
    pub mode: String,
    /// The timeline's schema.
    pub timeline_schema: String,
    /// The timeline file's SHA-256.
    pub timeline_sha256: String,
    /// The configuration's hash.
    pub config_hash: String,
    /// The configuration's name.
    pub name: String,
    /// The run's model identity (e.g., `field-ecology-v1`, `field-ecology-v2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Enabled scientific `field-ecology-v2` feature slugs, sorted; `None` on
    /// `field-ecology-v1*`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v2_features: Option<Vec<String>>,
    /// Whether `V2-16` (`v2-visual-mappings`, `INV-09`) was enabled on a
    /// `field-ecology-v2*` model; `None` on `field-ecology-v1*`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v2_visual_mappings: Option<bool>,
    /// Checkpoint / run-directory provenance when exported from a resumed
    /// Studio timeline (`None` for a founding session).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<TimelineStart>,
    /// Its seed.
    pub seed: u64,
    /// The ticks of the first and last frames.
    pub ticks: [u64; 2],
    /// The tick the session closed on.
    pub end_tick: u64,
    /// Committed ticks between consecutive sampled frames in
    /// `simulation-time` mode; `None` in `live-capture` mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_every_ticks: Option<u64>,
    /// Keyframe count of the Cinema [`CameraPath`] used, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_keys: Option<usize>,
    /// Simulated seconds a tick.
    pub seconds_per_tick: f64,
    /// The world's digest at `end_tick`, which the replay reached.
    pub digest: String,
    /// The commands the session applied.
    pub commands: usize,
    /// The route the frames were drawn by.
    pub renderer: String,
    /// How the renderer's linear colour became the frames' bytes.
    pub tone_mapping: String,
    /// The fixed projection the frames' affinity and composition colours
    /// were drawn through (plan §13.4's recorded matrix and scaling).
    pub projection: TimelineProjection,
    /// Distinct organism colour mappings rendered across exported frames, in
    /// first-appearance order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub color_mappings: Vec<String>,
    /// Distinct field mappings rendered across exported frames, in
    /// first-appearance order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_mappings: Vec<String>,
    /// Honest FL-043 / INV-15 peak memory admitted for this export (world +
    /// observers + PNG/NV12/y4m encoder scratch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admitted_peak_bytes: Option<u64>,
    /// What wrote the frames.
    pub frame_encoder: String,
    /// Wall seconds from the first frame to the last.
    pub wall_seconds: f64,
    /// Ticks shown a wall second over the frames, when they span time.
    pub ticks_per_wall_second: Option<f64>,
    /// Every frame drawn.
    pub frames: Vec<ExportedFrame>,
    /// The video, unless withheld.
    pub video: Option<ExportedVideo>,
    /// Why the video was withheld, if it was.
    pub video_withheld: Option<String>,
    /// The video encoded as H.264, when asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoded: Option<ExportedEncoding>,
}

/// One frame the export plans to emit.
struct Planned {
    frame: u64,
    tick: u64,
    wall_seconds: f64,
    drawable: [u32; 2],
}

/// Export the timeline `text` of `config`'s world into the new directory
/// `out` in live-capture mode (see the module documentation), and return its
/// manifest.
///
/// # Errors
/// As [`export_cinema`].
pub fn export(
    config: &ValidatedConfig,
    text: &str,
    options: ExportOptions,
    out: &Path,
) -> Result<ExportManifest, CoreError> {
    export_cinema(config, text, &CinemaOptions::from(options), out)
}

/// Export the timeline `text` of `config`'s world into the new directory
/// `out` under [`CinemaOptions`] (FL-023, FL-050, FL-043, G10): supports
/// live-capture and exact simulation-time sampling, 1080p/4K target
/// resolutions, optional keyframed [`CameraPath`], V2-16 visual mapping
/// propagation, and upfront FL-043 peak-memory admission before creating `out`.
///
/// # Errors
/// - [`CoreError::StorageFailed`] when `out` exists or a file cannot be
///   written.
/// - [`CoreError::StateConflict`] for an invalid timeline, disallowed v2
///   visual styles, non-monotonic ticks or wall times, or an MP4 asked of an
///   export whose video is withheld.
/// - [`CoreError::NumericalInvalid`] for invalid resolution or pixel density.
/// - [`CoreError::BudgetExhausted`] before creating `out` when the export's
///   world, observers, and encoder scratch exceed `[execution]
///   maximum_total_bytes` (`INV-15`).
/// - [`CoreError::MissingCapability`] before creating `out` without a Metal
///   device or without a runnable ffmpeg listing libx264 (when `mp4` is set).
pub fn export_cinema(
    config: &ValidatedConfig,
    text: &str,
    options: &CinemaOptions,
    out: &Path,
) -> Result<ExportManifest, CoreError> {
    if out.exists() {
        return Err(storage(
            "create the export directory",
            out,
            &std::io::Error::from(std::io::ErrorKind::AlreadyExists),
        ));
    }
    let target_resolution = options.resolved_target()?;
    let records = read_timeline(text)?;
    let validated = validate_timeline(config, &records)?;
    let seconds_per_tick = config.report().timestep.seconds_per_tick();

    let (planned, counts, mode_name, sample_every_ticks) = match options.mode {
        ExportMode::LiveCapture => {
            let planned: Vec<Planned> = validated
                .draws
                .iter()
                .map(|draw| Planned {
                    frame: draw.frame,
                    tick: draw.tick,
                    wall_seconds: draw.wall_seconds,
                    drawable: target_resolution.map_or(draw.drawable, |(size, _)| size),
                })
                .collect();
            let walls: Vec<f64> = planned.iter().map(|frame| frame.wall_seconds).collect();
            let counts = slot_counts(&walls, options.frames_per_second)?;
            (planned, counts, "live-capture", None)
        }
        ExportMode::SimulationTime { every_ticks } => {
            let samples = plan_simulation_samples(
                validated.opened,
                validated.end_tick,
                every_ticks,
                seconds_per_tick,
                options.frames_per_second,
            )?;
            let mut resize_by_tick: Vec<(u64, [u32; 2])> = Vec::new();
            let mut rec_tick = validated.opened;
            for record in &records {
                match record {
                    TimelineRecord::Show { current, .. }
                    | TimelineRecord::Place { current, .. }
                    | TimelineRecord::Advance { current, .. } => rec_tick = *current,
                    TimelineRecord::Resize { drawable } => {
                        resize_by_tick.push((rec_tick, *drawable));
                    }
                    _ => {}
                }
            }
            let mut cursor = 0_usize;
            let mut cur_drawable = validated.size;
            let mut planned = Vec::with_capacity(samples.len());
            for sample in samples {
                while cursor < resize_by_tick.len() && resize_by_tick[cursor].0 <= sample.tick {
                    cur_drawable = resize_by_tick[cursor].1;
                    cursor += 1;
                }
                planned.push(Planned {
                    frame: sample.frame,
                    tick: sample.tick,
                    wall_seconds: sample.video_seconds,
                    drawable: target_resolution.map_or(cur_drawable, |(size, _)| size),
                });
            }
            let counts = vec![1_u64; planned.len()];
            (planned, counts, "simulation-time", Some(every_ticks.get()))
        }
    };

    let withheld = match planned.first() {
        None => Some("no frame was drawn".to_owned()),
        Some(first) => planned
            .iter()
            .find(|frame| frame.drawable != first.drawable)
            .map(|frame| {
                format!(
                    "the drawable changed from {:?} to {:?} at frame {}, and a y4m stream keeps \
                     one size",
                    first.drawable, frame.drawable, frame.frame
                )
            })
            .or_else(|| {
                first
                    .drawable
                    .iter()
                    .any(|n| n % 2 == 1)
                    .then(|| format!("{:?} is not even, as 4:2:0 needs", first.drawable))
            }),
    };
    // Check external H.264 encoder before creating `out` or running replay.
    let ffmpeg = if options.mp4 {
        if let Some(why) = &withheld {
            return Err(conflict(format!(
                "an MP4 encodes the export's y4m video, which this export withholds: {why}"
            )));
        }
        Some(flife_cli::video_encoder()?)
    } else {
        None
    };

    let with_video = withheld.is_none();
    let extra_render_bytes = match target_resolution {
        Some((size, _)) => export_scratch_bytes(size, with_video),
        None => planned
            .iter()
            .map(|frame| export_scratch_bytes(frame.drawable, with_video))
            .max()
            .unwrap_or_else(|| export_scratch_bytes(validated.size, with_video)),
    };

    let video_path = out.join(VIDEO_FILE);
    let mut encoder = PngEncoder::new();
    let mut frames = Vec::with_capacity(planned.len());
    let mut color_mappings: Vec<String> = Vec::new();
    let mut field_mappings: Vec<String> = Vec::new();

    let replay_cfg = ReplayConfig {
        cpu_workers: options.cpu_workers,
        target_resolution,
        extra_render_bytes,
        camera: options.camera.as_ref(),
        sample_every_ticks: match options.mode {
            ExportMode::LiveCapture => None,
            ExportMode::SimulationTime { every_ticks } => {
                Some((every_ticks, options.frames_per_second))
            }
        },
    };
    let (replayed, admitted, video) = replay_with(
        config,
        &records,
        &replay_cfg,
        |_admitted| {
            std::fs::create_dir(out)
                .map_err(|error| storage("create the export directory", out, &error))?;
            if let (None, Some(first)) = (&withheld, planned.first()) {
                Ok(Some(Video::create(
                    &video_path,
                    first.drawable,
                    options.frames_per_second,
                )?))
            } else {
                Ok(None)
            }
        },
        |video, frame| {
            let index = frames.len();
            let plan = planned
                .get(index)
                .ok_or_else(|| conflict(format!("frame {} was not planned", frame.frame)))?;
            let layout = frame.pixels.layout();
            let [width, height] = plan.drawable;
            let rgba = frame.pixels.as_bytes();
            let expected_bytes = u64::from(width)
                .checked_mul(u64::from(height))
                .and_then(|pixels| pixels.checked_mul(4))
                .and_then(|bytes| usize::try_from(bytes).ok());
            if layout.format() != PixelFormat::Rgba8
                || [layout.width(), layout.height()] != plan.drawable
                || expected_bytes != Some(rgba.len())
            {
                return Err(CoreError::LayoutMismatch {
                    buffer: "exported frame",
                    detail: format!(
                        "frame {} is {:?} {}x{} in {} bytes; its drawable is {:?}",
                        frame.frame,
                        layout.format(),
                        layout.width(),
                        layout.height(),
                        rgba.len(),
                        plan.drawable
                    ),
                });
            }
            let png = encoder.encode_rgba8(width, height, rgba, CompressionLevel::Fast);
            let file = format!("frame-{index:06}.png");
            write_new(&out.join(&file), &png)?;
            let slots = counts[index];
            if let Some(video) = video.as_mut() {
                video.write(frame.pixels, slots)?;
            }
            let mapping_name = frame.style.mapping.name().to_owned();
            if !color_mappings.contains(&mapping_name) {
                color_mappings.push(mapping_name.clone());
            }
            let field_name = frame.style.field_mapping.name().to_owned();
            if !field_mappings.contains(&field_name) {
                field_mappings.push(field_name.clone());
            }
            #[expect(
                clippy::cast_precision_loss,
                reason = "tick counts far below 2^53 are exact in f64"
            )]
            let sim_seconds = frame.tick.get() as f64 * seconds_per_tick;
            frames.push(ExportedFrame {
                frame: frame.frame,
                tick: frame.tick.get(),
                wall_seconds: frame.wall_seconds,
                sim_seconds: Some(sim_seconds),
                mapping: Some(mapping_name),
                field_mapping: Some(field_name),
                camera_center: Some(frame.camera_center),
                camera_zoom: Some(frame.camera_zoom),
                file,
                sha256: Sha256Digest::of(&png).to_hex(),
                slots,
            });
            Ok(())
        },
    )?;

    if frames.len() != planned.len() {
        return Err(conflict(format!(
            "replay emitted {} frames, expected {}",
            frames.len(),
            planned.len()
        )));
    }
    let video = video
        .map(|video| video.finish(options.frames_per_second, &counts))
        .transpose()?;
    let encoded = match (ffmpeg, &video) {
        (Some(ffmpeg), Some(_)) => {
            let path = out.join(ENCODED_FILE);
            flife_cli::encode_y4m(&video_path, &path)?;
            Some(ExportedEncoding {
                file: ENCODED_FILE.to_owned(),
                source: VIDEO_FILE.to_owned(),
                ffmpeg,
                settings: flife_cli::Y4M_TO_H264.join(" "),
                sha256: file_sha256(&path)?,
            })
        }
        _ => None,
    };

    let first = planned.first();
    let last = planned.last();
    let wall_seconds = match (first, last) {
        (Some(first), Some(last)) => last.wall_seconds - first.wall_seconds,
        _ => 0.0,
    };
    let ticks = [first, last].map(|frame| frame.map_or(0, |frame| frame.tick));
    #[expect(
        clippy::cast_precision_loss,
        reason = "a tick count far below 2^53 is exact"
    )]
    let shown = ticks[1].saturating_sub(ticks[0]) as f64;
    let model_name = config.config().model.clone();
    let v2_features = config
        .enabled_v2_features()
        .map(|slugs| slugs.into_iter().map(str::to_owned).collect());
    let v2_visual_mappings =
        is_v2_model(&model_name).then(|| config.is_v2_feature_enabled(V2Feature::V2VisualMappings));
    let manifest = ExportManifest {
        schema: EXPORT_SCHEMA.to_owned(),
        mode: mode_name.to_owned(),
        timeline_schema: TIMELINE_SCHEMA.to_owned(),
        timeline_sha256: Sha256Digest::of(text.as_bytes()).to_hex(),
        config_hash: config.config_hash().to_hex(),
        name: validated.name,
        model: Some(model_name),
        v2_features,
        v2_visual_mappings,
        from: validated.from,
        seed: validated.seed,
        ticks,
        end_tick: replayed.tick.get(),
        sample_every_ticks,
        camera_keys: options.camera.as_ref().map(|path| path.keys.len()),
        seconds_per_tick,
        digest: replayed.digest,
        commands: replayed.commands,
        renderer: "fmn-render Metal render_three_d_rgba8".to_owned(),
        tone_mapping: "linear Rgba16F, clamped and sRGB-encoded to RGBA8 on the device \
                       (fmn-render srgb8_from_f16)"
            .to_owned(),
        projection: AffinityProjection::fixed().into(),
        color_mappings,
        field_mappings,
        admitted_peak_bytes: Some(admitted.peak_bytes),
        frame_encoder: "fmn-codec png, fast".to_owned(),
        wall_seconds,
        ticks_per_wall_second: (wall_seconds > 0.0).then(|| shown / wall_seconds),
        frames,
        video,
        video_withheld: withheld,
        encoded,
    };
    let mut json =
        serde_json::to_vec_pretty(&manifest).map_err(|error| CoreError::StateConflict {
            operation: "write an export manifest",
            detail: error.to_string(),
        })?;
    json.push(b'\n');
    write_new_synced(&out.join(MANIFEST_FILE), &json)?;
    Ok(manifest)
}

/// How many slots of a `fps` clock each frame fills, for frames drawn at
/// `walls` (in order): slot `k` at `walls[0] + k / fps` shows the last
/// frame drawn by then, up to the last frame's time.
///
/// # Errors
/// [`CoreError::StateConflict`] for more slots than a `u32` counts.
pub fn slot_counts(walls: &[f64], fps: NonZeroU32) -> Result<Vec<u64>, CoreError> {
    let mut counts = vec![0; walls.len()];
    let (Some(&first), Some(&last)) = (walls.first(), walls.last()) else {
        return Ok(counts);
    };
    if walls.iter().any(|wall| !wall.is_finite() || *wall < 0.0)
        || !walls.is_sorted_by(|a, b| a <= b)
    {
        return Err(conflict(
            "the frames' wall times are not finite and in order".into(),
        ));
    }
    let rate = f64::from(fps.get());
    let span = last - first;
    if !span.is_finite() || span * rate >= f64::from(u32::MAX) {
        return Err(conflict(format!(
            "more than {} slots at {fps} a second",
            u32::MAX
        )));
    }
    let mut shown = 0;
    let mut slot: u32 = 0;
    loop {
        let at = first + f64::from(slot) / rate;
        if at > last {
            return Ok(counts);
        }
        if slot > 0 && at <= first {
            return Err(conflict(format!(
                "frame wall time {first} is too large to advance by 1/{fps} s"
            )));
        }
        while walls.get(shown + 1).is_some_and(|&wall| wall <= at) {
            shown += 1;
        }
        counts[shown] += 1;
        slot = slot
            .checked_add(1)
            .ok_or_else(|| conflict(format!("more than {} slots at {fps} a second", u32::MAX)))?;
    }
}

/// The video being written.
struct Video {
    path: PathBuf,
    out: BufWriter<File>,
    hash: Sha256Stream,
    size: [u32; 2],
    nv12: FrameBuffer,
    record: Vec<u8>,
}

impl Video {
    fn create(path: &Path, size: [u32; 2], fps: NonZeroU32) -> Result<Self, CoreError> {
        let file =
            File::create_new(path).map_err(|error| storage("create the video", path, &error))?;
        let layout = FrameLayout::tight(PixelFormat::Nv12, size[0], size[1]).map_err(|error| {
            CoreError::LayoutMismatch {
                buffer: "export video frame",
                detail: error.to_string(),
            }
        })?;
        let mut video = Self {
            path: path.to_owned(),
            out: BufWriter::new(file),
            hash: Sha256Stream::new(),
            size,
            nv12: FrameBuffer::new(layout),
            record: Vec::new(),
        };
        let header = y4m_header(size[0], size[1], (fps.get(), 1), Y4mColorspace::C420Mpeg2);
        video.put(&header)?;
        Ok(video)
    }

    fn put(&mut self, bytes: &[u8]) -> Result<(), CoreError> {
        self.hash.update(bytes);
        self.out
            .write_all(bytes)
            .map_err(|error| storage("write the video", &self.path, &error))
    }

    /// Fill the next `slots` slots with `rgba`.
    fn write(&mut self, rgba: &FrameBuffer, slots: u64) -> Result<(), CoreError> {
        if slots == 0 {
            return Ok(());
        }
        rgba_to_nv12(
            rgba,
            &mut self.nv12,
            ColorRange::Limited,
            ChromaSiting::Left,
        )
        .map_err(|error| CoreError::LayoutMismatch {
            buffer: "export video frame",
            detail: error.to_string(),
        })?;
        self.record.clear();
        append_y4m_frame_nv12(&mut self.record, self.size[0], self.size[1], &self.nv12).map_err(
            |error| CoreError::LayoutMismatch {
                buffer: "export video frame",
                detail: error.to_string(),
            },
        )?;
        let record = std::mem::take(&mut self.record);
        for _ in 0..slots {
            self.put(&record)?;
        }
        self.record = record;
        Ok(())
    }

    fn finish(self, fps: NonZeroU32, counts: &[u64]) -> Result<ExportedVideo, CoreError> {
        let path = self.path;
        let file = self
            .out
            .into_inner()
            .map_err(|error| storage("write the video", &path, error.error()))?;
        file.sync_all()
            .map_err(|error| storage("write the video", &path, &error))?;
        Ok(ExportedVideo {
            file: VIDEO_FILE.to_owned(),
            encoder: "fmn-codec y4m".to_owned(),
            colorspace: "C420mpeg2".to_owned(),
            matrix: "bt709".to_owned(),
            range: "limited".to_owned(),
            chroma_siting: "left".to_owned(),
            size: self.size,
            frames_per_second: fps.get(),
            slots: counts.iter().sum(),
            repeated: counts.iter().map(|&count| count.saturating_sub(1)).sum(),
            dropped: counts.iter().filter(|&&count| count == 0).count() as u64,
            sha256: self.hash.finish().to_hex(),
        })
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    let mut file =
        File::create_new(path).map_err(|error| storage("create an export file", path, &error))?;
    file.write_all(bytes)
        .map_err(|error| storage("write an export file", path, &error))
}

/// Write `bytes` to a brand-new file and flush it durably (`sync_all`): used
/// for `manifest.json`, which marks an export complete.
fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    let mut file =
        File::create_new(path).map_err(|error| storage("create an export file", path, &error))?;
    file.write_all(bytes)
        .map_err(|error| storage("write an export file", path, &error))?;
    file.sync_all()
        .map_err(|error| storage("sync an export file", path, &error))
}

/// The SHA-256 of the file at `path`, read a block at a time.
fn file_sha256(path: &Path) -> Result<String, CoreError> {
    let mut file =
        File::open(path).map_err(|error| storage("read an export file", path, &error))?;
    let mut hash = Sha256Stream::new();
    let mut block = vec![0_u8; 1 << 16];
    loop {
        let read = file
            .read(&mut block)
            .map_err(|error| storage("read an export file", path, &error))?;
        if read == 0 {
            return Ok(hash.finish().to_hex());
        }
        hash.update(&block[..read]);
    }
}

fn storage(operation: &'static str, path: &Path, error: &std::io::Error) -> CoreError {
    CoreError::StorageFailed {
        operation,
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}

fn conflict(detail: String) -> CoreError {
    CoreError::StateConflict {
        operation: "export a studio timeline",
        detail,
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::time::{SystemTime, UNIX_EPOCH};

    use flife_core::{CoreError, ValidatedConfig};
    use flife_manim::Style;

    use super::{ExportOptions, export, slot_counts};
    use crate::timeline::{Exact, TimelineRecord, write_timeline_record};

    const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

    fn timeline_text(records: &[TimelineRecord]) -> String {
        let mut out = Vec::new();
        for record in records {
            write_timeline_record(&mut out, record).expect("serializes timeline record");
        }
        String::from_utf8(out).expect("utf-8 timeline")
    }

    #[test]
    fn export_rejects_backwards_frame_ticks_and_unsorted_walls_before_writing() {
        let config = ValidatedConfig::from_toml(PRESET).expect("valid preset");
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let out = std::env::temp_dir().join(format!(
            "flife-export-guard-{}-{unique}",
            std::process::id()
        ));
        let options = ExportOptions {
            frames_per_second: NonZeroU32::new(30).expect("non-zero fps"),
            cpu_workers: None,
            mp4: false,
        };
        let header = TimelineRecord::header(&config, [320, 200], Style::default());

        let backwards_ticks = timeline_text(&[
            header.clone(),
            TimelineRecord::Show {
                prior: None,
                current: 5,
                fraction: Exact(1.0),
            },
            TimelineRecord::Draw {
                frame: 0,
                tick: 5,
                wall_seconds: Exact(0.016),
            },
            TimelineRecord::Show {
                prior: Some(5),
                current: 3,
                fraction: Exact(1.0),
            },
            TimelineRecord::Draw {
                frame: 1,
                tick: 3,
                wall_seconds: Exact(0.032),
            },
            TimelineRecord::End {
                tick: 5,
                digest: String::new(),
                applied: Vec::new(),
            },
        ]);
        let err = export(&config, &backwards_ticks, options, &out)
            .expect_err("backwards frame ticks must be refused without panicking");
        assert!(
            matches!(err, CoreError::StateConflict { ref detail, .. } if detail.contains("tick")),
            "expected StateConflict on ticks, got {err:?}"
        );
        assert!(
            !out.exists(),
            "refused export must not create the output directory"
        );

        let backwards_walls = timeline_text(&[
            header,
            TimelineRecord::Show {
                prior: None,
                current: 1,
                fraction: Exact(1.0),
            },
            TimelineRecord::Draw {
                frame: 0,
                tick: 1,
                wall_seconds: Exact(0.050),
            },
            TimelineRecord::Show {
                prior: Some(1),
                current: 2,
                fraction: Exact(1.0),
            },
            TimelineRecord::Draw {
                frame: 1,
                tick: 2,
                wall_seconds: Exact(0.010),
            },
            TimelineRecord::End {
                tick: 2,
                digest: String::new(),
                applied: Vec::new(),
            },
        ]);
        let err = export(&config, &backwards_walls, options, &out)
            .expect_err("backwards wall times must be refused");
        assert!(
            matches!(err, CoreError::StateConflict { ref detail, .. } if detail.contains("wall times")),
            "expected StateConflict on wall times, got {err:?}"
        );
        assert!(!out.exists());

        let fps = NonZeroU32::new(10).expect("non-zero fps");
        assert_eq!(slot_counts(&[], fps).expect("empty"), Vec::<u64>::new());
        assert_eq!(slot_counts(&[1.0], fps).expect("single"), vec![1]);
        assert_eq!(
            slot_counts(&[0.0, 0.25], fps).expect("three slots"),
            vec![3, 0]
        );
        // Non-finite, negative, unsorted, overflow-span, and non-advancing wall
        // times must fail immediately rather than spinning up to u32::MAX slots.
        for bad in [
            &[f64::NAN][..],
            &[-0.1][..],
            &[1.0, 0.5][..],
            &[0.0, 1.0e12][..],
            &[1.0e20, 1.0e20][..],
        ] {
            assert!(
                matches!(slot_counts(bad, fps), Err(CoreError::StateConflict { .. })),
                "expected StateConflict for {bad:?}"
            );
        }
    }
}
