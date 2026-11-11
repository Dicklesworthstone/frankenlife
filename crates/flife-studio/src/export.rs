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
//! - `manifest.json`, written last, so a directory without one holds an
//!   export that did not finish. It records the timeline's and the
//!   configuration's hashes, the ticks shown and the tick the session
//!   closed on, the closing world's digest, the renderer, encoders,
//!   resolution, rate and tone mapping, and each file's SHA-256.
//!
//! The directory must not exist. An export never writes over a file.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};

use flife_core::{CoreError, Sha256Digest, Sha256Stream, ValidatedConfig};
use fmn_codec::{CompressionLevel, PngEncoder, Y4mColorspace, append_y4m_frame_nv12, y4m_header};
use fmn_frame::convert::rgba_to_nv12;
use fmn_frame::{ChromaSiting, ColorRange, FrameBuffer, FrameLayout, PixelFormat};
use serde::{Deserialize, Serialize};

use crate::timeline::{TIMELINE_SCHEMA, TimelineRecord, read_timeline, replay};

/// The schema of an export's manifest.
pub const EXPORT_SCHEMA: &str = "frankenlife.export.v1";

/// The manifest's file name in an export's directory.
pub const MANIFEST_FILE: &str = "manifest.json";

/// The video's file name in an export's directory.
pub const VIDEO_FILE: &str = "capture.y4m";

/// How a timeline is exported.
#[derive(Debug, Clone, Copy)]
pub struct ExportOptions {
    /// The video's constant rate.
    pub frames_per_second: NonZeroU32,
    /// Threads for the replayed world's field and organism phases; no
    /// result depends on it.
    pub cpu_workers: Option<NonZeroUsize>,
}

/// One frame of an export: a frame the Studio drew.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportedFrame {
    /// The Studio's index of the frame.
    pub frame: u64,
    /// The tick of the epoch it shows.
    pub tick: u64,
    /// When the Studio drew it, in seconds since it opened.
    pub wall_seconds: f64,
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
    /// Its seed.
    pub seed: u64,
    /// The ticks of the first and last frames.
    pub ticks: [u64; 2],
    /// The tick the session closed on.
    pub end_tick: u64,
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
}

/// One frame the timeline draws, as its records declare it.
struct Planned {
    frame: u64,
    tick: u64,
    wall_seconds: f64,
    drawable: [u32; 2],
}

/// Export the timeline `text` of `config`'s world into the new directory
/// `out` (see the module documentation), and return its manifest.
///
/// # Errors
/// - [`CoreError::StorageFailed`] when `out` exists or a file cannot be
///   written.
/// - [`CoreError::StateConflict`] for frame times that are not finite and
///   in order, for more slots than a `u32` counts, or for a frame of
///   another size than its drawable.
/// - As [`read_timeline`] and [`replay`].
pub fn export(
    config: &ValidatedConfig,
    text: &str,
    options: ExportOptions,
    out: &Path,
) -> Result<ExportManifest, CoreError> {
    let records = read_timeline(text)?;
    let Some(TimelineRecord::Header {
        name, seed, size, ..
    }) = records.first()
    else {
        return Err(conflict("a timeline opens with its header".into()));
    };
    let mut drawable = *size;
    let mut planned = Vec::new();
    for record in &records {
        match record {
            TimelineRecord::Resize { drawable: size } => drawable = *size,
            TimelineRecord::Draw {
                frame,
                tick,
                wall_seconds,
            } => planned.push(Planned {
                frame: *frame,
                tick: *tick,
                wall_seconds: wall_seconds.0,
                drawable,
            }),
            _ => {}
        }
    }
    let walls: Vec<f64> = planned.iter().map(|frame| frame.wall_seconds).collect();
    if walls.iter().any(|wall| !wall.is_finite()) || !walls.is_sorted_by(|a, b| a <= b) {
        return Err(conflict(
            "the frames' wall times are not finite and in order".into(),
        ));
    }
    let counts = slot_counts(&walls, options.frames_per_second)?;
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

    std::fs::create_dir(out)
        .map_err(|error| storage("create the export directory", out, &error))?;
    let video_path = out.join(VIDEO_FILE);
    let mut video = match (&withheld, planned.first()) {
        (None, Some(first)) => Some(Video::create(
            &video_path,
            first.drawable,
            options.frames_per_second,
        )?),
        _ => None,
    };
    let mut encoder = PngEncoder::new();
    let mut frames = Vec::with_capacity(planned.len());
    let replayed = replay(config, &records, options.cpu_workers, |frame| {
        let index = frames.len();
        let plan = planned
            .get(index)
            .ok_or_else(|| conflict(format!("frame {} was not planned", frame.frame)))?;
        let layout = frame.pixels.layout();
        let [width, height] = plan.drawable;
        let rgba = frame.pixels.as_bytes();
        if layout.format() != PixelFormat::Rgba8
            || [layout.width(), layout.height()] != plan.drawable
            || rgba.len() != width as usize * height as usize * 4
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
        if let Some(video) = &mut video {
            video.write(frame.pixels, slots)?;
        }
        frames.push(ExportedFrame {
            frame: frame.frame,
            tick: frame.tick.get(),
            wall_seconds: frame.wall_seconds,
            file,
            sha256: Sha256Digest::of(&png).to_hex(),
            slots,
        });
        Ok(())
    })?;
    let video = video
        .map(|video| video.finish(options.frames_per_second, &counts))
        .transpose()?;

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
    let shown = (ticks[1] - ticks[0]) as f64;
    let manifest = ExportManifest {
        schema: EXPORT_SCHEMA.to_owned(),
        mode: "live-capture".to_owned(),
        timeline_schema: TIMELINE_SCHEMA.to_owned(),
        timeline_sha256: Sha256Digest::of(text.as_bytes()).to_hex(),
        config_hash: config.config_hash().to_hex(),
        name: name.clone(),
        seed: *seed,
        ticks,
        end_tick: replayed.tick.get(),
        seconds_per_tick: config.report().timestep.seconds_per_tick(),
        digest: replayed.digest,
        commands: replayed.commands,
        renderer: "fmn-render Metal render_three_d_rgba8".to_owned(),
        tone_mapping: "linear Rgba16F, clamped and sRGB-encoded to RGBA8 on the device \
                       (fmn-render srgb8_from_f16)"
            .to_owned(),
        frame_encoder: "fmn-codec png, fast".to_owned(),
        wall_seconds,
        ticks_per_wall_second: (wall_seconds > 0.0).then(|| shown / wall_seconds),
        frames,
        video,
        video_withheld: withheld,
    };
    let mut json =
        serde_json::to_vec_pretty(&manifest).map_err(|error| CoreError::StateConflict {
            operation: "write an export manifest",
            detail: error.to_string(),
        })?;
    json.push(b'\n');
    write_new(&out.join(MANIFEST_FILE), &json)?;
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
    let rate = f64::from(fps.get());
    let mut shown = 0;
    let mut slot: u32 = 0;
    loop {
        let at = first + f64::from(slot) / rate;
        if at > last {
            return Ok(counts);
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
