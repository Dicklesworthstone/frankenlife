//! FL-023 (G10, plan §13.8, §21.3): a recorded Studio session replays
//! frame for frame. An offscreen Studio records its timeline while posted
//! input does the following:
//! - sets an overlay and restyles the view, its colours and its field's;
//! - zooms and resets the camera, selects an organism and has the camera
//!   track it (plan §14.3);
//! - makes a brush command;
//! - runs the world with the camera moving and the display between epochs;
//! - pauses and steps it.
//!
//! The timeline is written as NDJSON and read back. Its replay draws every
//! frame the Studio drew again, byte for byte, at the tick the Studio showed
//! it, and closes on the world the Studio closed on. A timeline that
//! disagrees with its world is refused.
//!
//! A recorded interval exports through FrankenManim's frame and encoding
//! paths. Each frame becomes a PNG of exactly the replay's bytes, and the
//! frames on a 30 fps clock become y4m video, with the repeats and drops
//! it declares. The manifest names its sources. An export never writes
//! over a directory. A drawable that 4:2:0 cannot take withholds the video
//! and says why. Each run leaves its exports, about 1 MB, under the
//! target's temporary directory; nothing deletes them.
//!
//! No window opens: headless Metal. Without a Metal device the tests
//! return early.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test: a failure is a test failure; figures are printed"
)]

use std::hash::{DefaultHasher, Hash, Hasher};
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use flife_core::{CoreError, Sha256Digest, ValidatedConfig};
use flife_manim::{Region, RegionEffect, RegionShape};
use flife_studio::{
    ColorMapping, ControllerChoice, Disk, EXPORT_SCHEMA, Exact, ExportManifest, ExportOptions,
    FieldMapping, KEY_C, KEY_DIGITS, KEY_E, KEY_G, KEY_PERIOD, KEY_RIGHT_BRACKET, KEY_SPACE, KEY_T,
    KEY_ZERO, Look, MANIFEST_FILE, Status, Studio, StudioOptions, Style, Surface, TimelineRecord,
    TimelineRegion, TimelineStyle, VIDEO_FILE, export, read_timeline, replay,
    write_timeline_record,
};
use fmn_codec::{
    PngLimits, Y4mColorspace, append_y4m_frame_nv12, decode_png, decode_y4m, y4m_header,
};
use fmn_frame::convert::rgba_to_nv12;
use fmn_frame::{ChromaSiting, ColorRange, FrameBuffer, FrameLayout, PixelFormat};
use fmn_render::metal::{
    InputEvent, LogicalPoint, MetalRenderer, Modifiers, PointerButton, ScrollUnit,
};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");
const SIZE: [u32; 2] = [320, 200];

fn config() -> ValidatedConfig {
    ValidatedConfig::from_toml(PRESET).expect("the preset is valid")
}

fn key(code: u16, text: &str) -> [InputEvent; 2] {
    [true, false].map(|pressed| InputEvent::Key {
        code,
        text: text.to_owned(),
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    })
}

fn click(at: [f64; 2]) -> [InputEvent; 2] {
    [true, false].map(|pressed| InputEvent::Button {
        button: PointerButton::Primary,
        pressed,
        position: LogicalPoint { x: at[0], y: at[1] },
        clicks: 1,
        modifiers: Modifiers::NONE,
    })
}

fn scroll(at: [f64; 2], delta: f64) -> InputEvent {
    InputEvent::Scroll {
        delta: [0.0, delta],
        unit: ScrollUnit::Points,
        position: LogicalPoint { x: at[0], y: at[1] },
        modifiers: Modifiers::NONE,
    }
}

fn digest(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// A recording Studio and what it has recorded: its timeline, with the
/// session's statuses in, and each drawn frame's tick and pixels' hash.
struct Recording {
    studio: Studio,
    records: Vec<TimelineRecord>,
    drawn: Vec<(u64, u64)>,
    statuses: Vec<Status>,
}

impl Recording {
    fn post(&mut self, events: impl IntoIterator<Item = InputEvent>) {
        for event in events {
            self.studio.post_input(event).expect("posts");
        }
    }

    fn frame(&mut self) {
        let frame = self.studio.frame().expect("a frame");
        self.records.extend(self.studio.take_recorded());
        for status in self.studio.take_statuses() {
            self.records.extend(TimelineRecord::status(&status));
            self.statuses.push(status);
        }
        if frame.drawn {
            let tick = frame.shown.expect("an epoch is shown").tick.get();
            let pixels = self.studio.last_frame().expect("the frame read back");
            self.drawn.push((tick, digest(pixels.as_bytes())));
        }
    }

    /// Frames until `done` holds for the statuses so far, within 30 s.
    fn until(&mut self, what: &str, done: impl Fn(&[Status]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done(&self.statuses) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.frame();
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn a_recorded_session_replays_frame_for_frame() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let config = config();
    let options = StudioOptions {
        size: Some(SIZE),
        cpu_workers: NonZeroUsize::new(2),
        controllers: ControllerChoice::Cpu,
        surface: Surface::Offscreen {
            scale: NonZeroU32::new(2).expect("nonzero"),
        },
        record: true,
        ..StudioOptions::default()
    };
    let studio = Studio::open(&config, &options).expect("opens offscreen");
    let mut recording = Recording {
        studio,
        records: Vec::new(),
        drawn: Vec::new(),
        statuses: Vec::new(),
    };
    let centre = [160.0, 100.0];

    recording.frame();
    recording
        .studio
        .set_overlay("FrankenLife ■ recorded", &[[230, 120, 40]])
        .expect("an overlay");
    recording.frame();
    // Key 3: the generation mapping.
    recording.post(key(KEY_DIGITS[2], "3"));
    recording.frame();
    // C: the field's composition (FL-019, plan §13.4).
    recording.post(key(KEY_C, "c"));
    recording.frame();
    // ] twice: the field's chosen component, channel 0, then channel 1.
    for _ in 0..2 {
        recording.post(key(KEY_RIGHT_BRACKET, "]"));
        recording.frame();
    }
    recording.post([scroll([100.0, 60.0], 40.0)]);
    recording.frame();
    // A click on the shown organism nearest the centre selects it.
    let density = 2.0;
    let viewport = *recording.studio.view().viewport();
    let distance = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let spot = recording
        .studio
        .view()
        .shown_rows()
        .map(|(_, position)| viewport.world_to_drawable(position))
        .min_by(|a, b| {
            let drawn = centre.map(|x| x * density);
            distance(*a, drawn).total_cmp(&distance(*b, drawn))
        })
        .expect("organisms are shown");
    recording.post(click(spot.map(|x| x / density)));
    let deadline = Instant::now() + Duration::from_secs(30);
    while recording.studio.view().selected().is_none() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the selection"
        );
        recording.frame();
    }
    // T: the camera tracks the selection from here, through the run, until
    // the reset (or until an epoch no longer draws it).
    recording.post(key(KEY_T, "t"));
    recording.frame();
    assert_eq!(recording.studio.tally().tracked, 1, "T centred the view");
    recording.post(key(KEY_E, "e"));
    recording.frame();
    recording.post(click([110.0, 70.0]));
    recording.until("the stroke's acceptance", |statuses| {
        statuses
            .iter()
            .any(|status| matches!(status, Status::Submitted { .. }))
    });
    // G: the measuring tool (plan §14.3), its disk placed by a click and
    // outlined in the measurement colour, then put away.
    recording.post(key(KEY_G, "g"));
    recording.post(click([150.0, 90.0]));
    recording.until("the measurement", |statuses| {
        statuses
            .iter()
            .any(|status| matches!(status, Status::Measured { .. }))
    });
    recording.frame();
    recording.post(key(KEY_G, "g"));
    recording.frame();
    // Run, the camera moving every fifth frame, then pause.
    recording.post(key(KEY_SPACE, " "));
    for index in 0..48 {
        if index % 5 == 0 {
            let delta = if index % 10 == 0 { 12.0 } else { -9.0 };
            recording.post([scroll(centre, delta)]);
        }
        recording.frame();
        std::thread::sleep(Duration::from_millis(12));
    }
    recording.post(key(KEY_SPACE, " "));
    recording.until("the pause", |statuses| {
        statuses
            .iter()
            .any(|status| matches!(status, Status::Paused { .. }))
    });
    for _ in 0..6 {
        recording.frame();
        std::thread::sleep(Duration::from_millis(12));
    }
    recording.post(key(KEY_PERIOD, "."));
    let stepped = recording.statuses.len();
    recording.until("the step", |statuses| {
        statuses[stepped..]
            .iter()
            .any(|status| matches!(status, Status::Stepped { .. }))
    });
    recording.post(key(KEY_ZERO, "0"));
    for _ in 0..4 {
        recording.frame();
    }
    let Recording {
        mut studio,
        mut records,
        drawn,
        ..
    } = recording;
    records.extend(studio.take_recorded());
    let closed = studio.close().expect("closes");
    records.push(TimelineRecord::end(&closed.session));

    // Every kind of record was made, and the display moved between epochs.
    let count = |which: fn(&TimelineRecord) -> bool| records.iter().filter(|r| which(r)).count();
    let kinds = [
        (
            "header",
            count(|r| matches!(r, TimelineRecord::Header { .. })),
        ),
        (
            "resize",
            count(|r| matches!(r, TimelineRecord::Resize { .. })),
        ),
        (
            "density",
            count(|r| matches!(r, TimelineRecord::Density { .. })),
        ),
        (
            "overlay",
            count(|r| matches!(r, TimelineRecord::Overlay { .. })),
        ),
        (
            "style",
            count(|r| matches!(r, TimelineRecord::Style { .. })),
        ),
        ("view", count(|r| matches!(r, TimelineRecord::View { .. }))),
        (
            "select",
            count(|r| matches!(r, TimelineRecord::Select { .. })),
        ),
        ("show", count(|r| matches!(r, TimelineRecord::Show { .. }))),
        (
            "place",
            count(|r| matches!(r, TimelineRecord::Place { .. })),
        ),
        (
            "advance",
            count(|r| matches!(r, TimelineRecord::Advance { .. })),
        ),
        (
            "submitted",
            count(|r| matches!(r, TimelineRecord::Submitted { .. })),
        ),
        (
            "applied",
            count(|r| matches!(r, TimelineRecord::Applied { .. })),
        ),
        (
            "region",
            count(|r| matches!(r, TimelineRecord::Region { region: Some(_) })),
        ),
        (
            "measuring region",
            count(|r| {
                matches!(
                    r,
                    TimelineRecord::Region {
                        region: Some(TimelineRegion::Disc {
                            removes: false,
                            measures: true,
                            ..
                        })
                    }
                )
            }),
        ),
    ];
    println!(
        "{} records, {} frames drawn: {kinds:?}",
        records.len(),
        drawn.len()
    );
    for (kind, made) in kinds {
        assert!(made > 0, "no {kind} record: {kinds:?}");
    }
    assert!(
        records.iter().any(|record| matches!(
            record,
            TimelineRecord::Style { style } if style.field_mapping == "composition"
        )),
        "the C key's field mapping was recorded"
    );
    for name in ["component-0", "component-1"] {
        assert!(
            records.iter().any(|record| matches!(
                record,
                TimelineRecord::Style { style } if style.field_mapping == name
            )),
            "the ] key's {name} was recorded"
        );
    }
    assert_eq!(
        count(|r| matches!(r, TimelineRecord::Draw { .. })),
        drawn.len()
    );
    assert!(
        records.iter().any(|record| matches!(
            record,
            TimelineRecord::Advance { prior: Some(_), fraction: Exact(f), .. } if 0.0 < *f && *f < 1.0
        )),
        "the display moved between epochs"
    );
    assert!(
        records
            .iter()
            .any(|record| matches!(record, TimelineRecord::Select { organism: Some(_) })),
        "an organism was selected"
    );
    let TimelineRecord::End {
        tick,
        digest: closed_digest,
        applied,
    } = records.last().expect("the end").clone()
    else {
        panic!("the last record is the end");
    };
    assert_eq!(applied.len(), 1, "the stroke applied");
    assert_eq!(closed_digest, closed.session.world.digest().to_hex());

    // As a file: written, read back, the same records.
    let text: String = records
        .iter()
        .map(|record| serde_json::to_string(record).expect("a record") + "\n")
        .collect();
    let read = read_timeline(&text).expect("reads");
    assert_eq!(read, records, "the timeline reads back exactly");

    let mut replayed_frames = 0_usize;
    let replayed = replay(&config, &read, NonZeroUsize::new(2), |frame| {
        let index = usize::try_from(frame.frame).expect("an index");
        assert_eq!(index, replayed_frames, "frames in order");
        let (shown, hash) = drawn[index];
        assert_eq!(frame.tick.get(), shown, "frame {index}'s tick");
        assert_eq!(
            digest(frame.pixels.as_bytes()),
            hash,
            "frame {index}'s pixels"
        );
        replayed_frames += 1;
        Ok(())
    })
    .expect("replays");
    assert_eq!(replayed_frames, drawn.len());
    assert_eq!(replayed.frames, drawn.len() as u64);
    assert_eq!(
        (replayed.tick.get(), &replayed.digest),
        (tick, &closed_digest)
    );
    assert_eq!(replayed.commands, 1);

    // A timeline that disagrees with its world is refused: another closing
    // world, a command moved a tick, a frame dated to another epoch than
    // the one shown, another configuration.
    let refused = |records: &[TimelineRecord], config: &ValidatedConfig| match replay(
        config,
        records,
        NonZeroUsize::new(2),
        |_| Ok(()),
    ) {
        Err(CoreError::StateConflict { detail, .. }) => detail,
        other => panic!("not refused: {other:?}"),
    };
    let header = read[0].clone();
    let mut wrong = read.last().expect("the end").clone();
    if let TimelineRecord::End { digest, .. } = &mut wrong {
        digest.replace_range(0..1, if digest.starts_with('0') { "1" } else { "0" });
    }
    println!("{}", refused(&[header.clone(), wrong], &config));
    let mut late = read.last().expect("the end").clone();
    if let TimelineRecord::End { applied, .. } = &mut late {
        applied[0].tick += 1;
    }
    println!("{}", refused(&[header.clone(), late], &config));
    let mut misdated = read.clone();
    let draw = misdated
        .iter_mut()
        .find_map(|record| match record {
            TimelineRecord::Draw { tick, .. } if *tick > 0 => Some(tick),
            _ => None,
        })
        .expect("a frame after the founding");
    *draw -= 1;
    println!("{}", refused(&misdated, &config));
    let other = ValidatedConfig::from_toml(&PRESET.replacen("seed = 42", "seed = 43", 1))
        .expect("another seed");
    println!("{}", refused(&read, &other));
}

/// A new directory's path under the target's temporary directory.
fn fresh(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after the epoch")
        .as_nanos();
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("fl023-{name}-{}-{nanos}", std::process::id()))
}

/// The y4m record of `rgba` as an export writes it: BT.709, limited range,
/// left siting.
fn y4m_frame(rgba: &FrameBuffer) -> Vec<u8> {
    let layout = rgba.layout();
    let mut nv12 = FrameBuffer::new(
        FrameLayout::tight(PixelFormat::Nv12, layout.width(), layout.height()).expect("even"),
    );
    rgba_to_nv12(rgba, &mut nv12, ColorRange::Limited, ChromaSiting::Left).expect("converts");
    let mut record = Vec::new();
    append_y4m_frame_nv12(&mut record, layout.width(), layout.height(), &nv12).expect("a record");
    record
}

/// A style record names its field mapping; one from before FL-019's
/// composition mapping has none and means the energy field. A name that is
/// not a field mapping is refused.
#[test]
fn a_style_record_names_its_field_mapping() {
    let older: TimelineStyle = serde_json::from_str(
        r#"{"mapping":"generation","look":"glow","particles":"points","field":true}"#,
    )
    .expect("an older style record");
    assert_eq!(older.field_mapping, "energy");
    assert_eq!(
        older.style().expect("a style"),
        Style {
            mapping: ColorMapping::Generation,
            ..Style::default()
        }
    );
    for field_mapping in FieldMapping::ALL {
        let style = Style {
            look: Look::Raw,
            field_mapping,
            ..Style::default()
        };
        let line = serde_json::to_string(&TimelineStyle::from(style)).expect("written");
        assert!(
            line.contains(&format!("\"field_mapping\":\"{field_mapping}\"")),
            "{line}"
        );
        let read: TimelineStyle = serde_json::from_str(&line).expect("read");
        assert_eq!(read.style().expect("a style"), style);
    }
    let wrong = TimelineStyle {
        field_mapping: "chemistry".to_owned(),
        ..TimelineStyle::from(Style::default())
    };
    assert!(matches!(
        wrong.style(),
        Err(CoreError::NumericalInvalid { .. })
    ));
}

/// A region record says whether it outlines a measurement (plan §14.3):
/// the measuring tool's disk reads back as itself, a stroke's record is
/// written as before the measuring tool, with no `measures` field, and an
/// older record without one reads as a stroke's. A record that both removes
/// and measures is refused.
#[test]
fn a_region_record_says_whether_it_measures() {
    let disk = Disk {
        center: [12.5, 3.25],
        radius: 1.5,
    };
    let measured = disk.region();
    let line = serde_json::to_string(&TimelineRegion::from(measured)).expect("written");
    // Exact: each figure its bits in hexadecimal.
    assert_eq!(
        line,
        r#"{"shape":"disc","center":["4029000000000000","400a000000000000"],"radius":"3ff8000000000000","removes":false,"measures":true}"#
    );
    let read: TimelineRegion = serde_json::from_str(&line).expect("read");
    assert_eq!(Region::try_from(read).expect("a region"), measured);

    let removal = Region {
        shape: RegionShape::Disc {
            center: disk.center,
            radius: disk.radius,
        },
        effect: RegionEffect::Removes,
    };
    let cell = Region {
        shape: RegionShape::Rectangle([[1.0, 2.0], [3.0, 4.0]]),
        effect: RegionEffect::Adds,
    };
    for (stroke, written) in [
        (
            removal,
            r#"{"shape":"disc","center":["4029000000000000","400a000000000000"],"radius":"3ff8000000000000","removes":true}"#,
        ),
        (
            cell,
            r#"{"shape":"rectangle","bounds":[["3ff0000000000000","4000000000000000"],["4008000000000000","4010000000000000"]],"removes":false}"#,
        ),
    ] {
        let line = serde_json::to_string(&TimelineRegion::from(stroke)).expect("written");
        assert_eq!(line, written);
        let read: TimelineRegion = serde_json::from_str(&line).expect("read");
        assert_eq!(Region::try_from(read).expect("a region"), stroke);
    }

    let both: TimelineRegion = serde_json::from_str(
        r#"{"shape":"disc","center":["4029000000000000","400a000000000000"],"radius":"3ff8000000000000","removes":true,"measures":true}"#,
    )
    .expect("a record");
    assert!(matches!(
        Region::try_from(both),
        Err(CoreError::StateConflict { .. })
    ));
}

#[test]
fn a_recorded_interval_exports_its_frames_and_video() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let config = config();
    let options = StudioOptions {
        size: Some([96, 60]),
        cpu_workers: NonZeroUsize::new(2),
        run_on_open: true,
        controllers: ControllerChoice::Cpu,
        surface: Surface::Offscreen {
            scale: NonZeroU32::new(2).expect("nonzero"),
        },
        record: true,
        ..StudioOptions::default()
    };
    let mut studio = Studio::open(&config, &options).expect("opens offscreen");
    let mut text = Vec::new();
    let mut write = |records: &[TimelineRecord]| {
        for record in records {
            write_timeline_record(&mut text, record).expect("writes");
        }
    };
    // Frames closer than the clock's slots and farther apart: drops and
    // repeats.
    for index in 0..24 {
        studio.frame().expect("a frame");
        if index == 2 {
            studio
                .set_overlay("exported ■", &[[40, 200, 120]])
                .expect("an overlay");
        }
        let statuses: Vec<_> = studio.take_statuses().collect();
        write(&studio.take_recorded());
        write(
            &statuses
                .iter()
                .filter_map(TimelineRecord::status)
                .collect::<Vec<_>>(),
        );
        std::thread::sleep(Duration::from_millis(if index % 6 == 5 { 90 } else { 8 }));
    }
    write(&studio.take_recorded());
    let closed = studio.close().expect("closes");
    write(&[TimelineRecord::end(&closed.session)]);
    let text = String::from_utf8(text).expect("utf-8");

    // What the replay draws, frame by frame.
    let mut drawn: Vec<(u64, f64, FrameBuffer)> = Vec::new();
    let records = read_timeline(&text).expect("reads");
    replay(&config, &records, NonZeroUsize::new(2), |frame| {
        drawn.push((frame.tick.get(), frame.wall_seconds, frame.pixels.clone()));
        Ok(())
    })
    .expect("replays");

    let out = fresh("export");
    let fps = NonZeroU32::new(30).expect("nonzero");
    let manifest = export(
        &config,
        &text,
        ExportOptions {
            frames_per_second: fps,
            cpu_workers: NonZeroUsize::new(2),
        },
        &out,
    )
    .expect("exports");
    println!(
        "{} frames, video {:?}, ticks {:?} to {}, into {}",
        manifest.frames.len(),
        manifest
            .video
            .as_ref()
            .map(|video| (video.slots, video.repeated, video.dropped)),
        manifest.ticks,
        manifest.end_tick,
        out.display()
    );
    assert_eq!(manifest.schema, EXPORT_SCHEMA);
    assert_eq!(manifest.mode, "live-capture");
    assert_eq!(
        manifest.timeline_sha256,
        Sha256Digest::of(text.as_bytes()).to_hex()
    );
    assert_eq!(manifest.config_hash, config.config_hash().to_hex());
    assert_eq!(manifest.digest, closed.session.world.digest().to_hex());
    assert_eq!(manifest.end_tick, closed.session.boundary.tick.get());
    assert!(manifest.ticks[1] > manifest.ticks[0], "the world ran");
    assert_eq!(manifest.frames.len(), drawn.len());

    // Each frame: a PNG of the replay's bytes, as the manifest hashes it.
    for (index, (frame, (tick, wall, pixels))) in manifest.frames.iter().zip(&drawn).enumerate() {
        assert_eq!(frame.file, format!("frame-{index:06}.png"));
        assert_eq!((frame.tick, frame.wall_seconds), (*tick, *wall));
        let png = std::fs::read(out.join(&frame.file)).expect("the frame's file");
        assert_eq!(Sha256Digest::of(&png).to_hex(), frame.sha256);
        let decoded = decode_png(&png, &PngLimits::default()).expect("decodes");
        let layout = pixels.layout();
        assert_eq!(
            [decoded.width, decoded.height],
            [layout.width(), layout.height()]
        );
        assert_eq!(decoded.rgba, pixels.as_bytes(), "frame {index}'s pixels");
    }

    // The clock: slot k at the first frame's time plus k/30 s shows the
    // last frame drawn by then, counted here afresh.
    let walls: Vec<f64> = drawn.iter().map(|(_, wall, _)| *wall).collect();
    let mut expected = vec![0_u64; walls.len()];
    for slot in 0_u32.. {
        let at = walls[0] + f64::from(slot) / 30.0;
        if at > walls[walls.len() - 1] {
            break;
        }
        let shown = walls
            .iter()
            .rposition(|&wall| wall <= at)
            .expect("the first");
        expected[shown] += 1;
    }
    let slots: Vec<u64> = manifest.frames.iter().map(|frame| frame.slots).collect();
    assert_eq!(slots, expected, "each frame's slots");
    let video = manifest.video.as_ref().expect("a video");
    assert_eq!(video.slots, expected.iter().sum::<u64>());
    assert_eq!(
        video.repeated,
        expected.iter().map(|&n| n.saturating_sub(1)).sum::<u64>()
    );
    assert_eq!(
        video.dropped,
        expected.iter().filter(|&&n| n == 0).count() as u64
    );
    assert!(video.repeated > 0 && video.dropped > 0, "{video:?}");
    let [width, height] = video.size;
    assert_eq!(
        (width, height, video.frames_per_second),
        (192, 120, 30),
        "96 x 60 points at 2 pixels a point"
    );

    // The video, byte for byte: its header, then each slot's frame
    // converted.
    let bytes = std::fs::read(out.join(VIDEO_FILE)).expect("the video");
    assert_eq!(Sha256Digest::of(&bytes).to_hex(), video.sha256);
    let mut ours = y4m_header(width, height, (30, 1), Y4mColorspace::C420Mpeg2);
    for ((_, _, pixels), &count) in drawn.iter().zip(&expected) {
        let record = y4m_frame(pixels);
        for _ in 0..count {
            ours.extend_from_slice(&record);
        }
    }
    assert!(bytes == ours, "the video is the slots' frames");
    let decoded = decode_y4m(&bytes).expect("decodes");
    assert_eq!(
        (
            decoded.width,
            decoded.height,
            decoded.fps,
            decoded.frames.len() as u64
        ),
        (width, height, (30, 1), video.slots)
    );

    // The manifest on disk, written last.
    let written: ExportManifest =
        serde_json::from_slice(&std::fs::read(out.join(MANIFEST_FILE)).expect("the manifest"))
            .expect("parses");
    assert_eq!(
        (
            &written.digest,
            &written.timeline_sha256,
            written.frames.len()
        ),
        (
            &manifest.digest,
            &manifest.timeline_sha256,
            manifest.frames.len()
        )
    );
    assert_eq!(
        written.video.map(|video| video.sha256),
        Some(video.sha256.clone())
    );

    // An export never writes over one.
    let before = std::fs::read(out.join(MANIFEST_FILE)).expect("the manifest");
    match export(
        &config,
        &text,
        ExportOptions {
            frames_per_second: fps,
            cpu_workers: NonZeroUsize::new(2),
        },
        &out,
    ) {
        Err(CoreError::StorageFailed { .. }) => {}
        other => panic!("not refused: {other:?}"),
    }
    assert_eq!(
        std::fs::read(out.join(MANIFEST_FILE)).expect("the manifest"),
        before
    );

    // A drawable 4:2:0 cannot take: the frames, and no video, with why.
    let odd: String = records
        .iter()
        .map(|record| match record {
            TimelineRecord::Resize { .. } => TimelineRecord::Resize {
                drawable: [191, 120],
            },
            other => other.clone(),
        })
        .map(|record| serde_json::to_string(&record).expect("a record") + "\n")
        .collect();
    let out = fresh("odd");
    let manifest = export(
        &config,
        &odd,
        ExportOptions {
            frames_per_second: fps,
            cpu_workers: NonZeroUsize::new(2),
        },
        &out,
    )
    .expect("exports its frames");
    println!("{:?}", manifest.video_withheld);
    assert!(manifest.video.is_none());
    assert!(
        manifest
            .video_withheld
            .as_deref()
            .is_some_and(|why| why.contains("not even")),
        "{:?}",
        manifest.video_withheld
    );
    assert!(!out.join(VIDEO_FILE).exists(), "no partial video");
    let png = std::fs::read(out.join(&manifest.frames[0].file)).expect("a frame");
    let decoded = decode_png(&png, &PngLimits::default()).expect("decodes");
    assert_eq!([decoded.width, decoded.height], [191, 120]);
}
