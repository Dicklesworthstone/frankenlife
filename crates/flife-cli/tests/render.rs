//! `flife render`: pre-rendered frames of the reference world through the
//! Studio's FrankenManim view, without perturbing the world, and with
//! `--video` an H.264 MP4 of them in BT.709 limited range.

use std::num::NonZeroUsize;

use flife_cli::{
    CameraPath, CameraView, ColorMapping, ControllerChoice, FieldComponent, FieldMapping, Look,
    Particles, RENDER_SCHEMA, RenderOptions, RenderProjection, RenderRecord, Style, render,
};
use flife_core::ValidatedConfig;
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use fmn_codec::append_y4m_frame_nv12;
use fmn_codec::png::{PngLimits, decode};
use fmn_frame::convert::rgba_to_nv12;
use fmn_frame::{ChromaSiting, ColorRange, FrameBuffer, FrameLayout, PixelFormat};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

#[test]
fn rendering_writes_png_frames_and_leaves_the_world_as_a_plain_run_leaves_it() {
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("flife-render-test");
    std::fs::create_dir_all(&out).expect("a scratch directory");
    let options = RenderOptions {
        ticks: 6,
        every: 3,
        size: [96, 64],
        out: out.clone(),
        cpu_workers: NonZeroUsize::new(3),
        controllers: ControllerChoice::Auto,
        render_threads: NonZeroUsize::MIN,
        video: None,
        fps: 30,
        style: Style::default(),
        camera: None,
    };
    let mut records = Vec::new();
    render(&config, &options, |record| records.push(record.clone())).expect("renders");

    let frames: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            RenderRecord::Frame {
                schema,
                frame,
                tick,
                path,
                ..
            } => Some((*schema, *frame, *tick, path.clone())),
            RenderRecord::Start { .. } | RenderRecord::End { .. } => None,
        })
        .collect();
    assert_eq!(
        frames.iter().map(|f| (f.1, f.2)).collect::<Vec<_>>(),
        [(0, 0), (1, 3), (2, 6)],
        "a frame at the founding and every third tick"
    );
    for (schema, _, _, path) in &frames {
        assert_eq!(*schema, RENDER_SCHEMA);
        let png = std::fs::read(path).expect("the frame exists");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "{path} is a PNG");
        // IHDR: width and height, big-endian, after the signature and the
        // chunk's length and type.
        assert_eq!(&png[16..24], &[0, 0, 0, 96, 0, 0, 0, 64]);
    }

    // Rendering only reads epochs: the world is the one a plain run of six
    // ticks on one thread commits.
    let mut plain = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    for _ in 0..6 {
        assert!(matches!(
            plain.step().expect("steps"),
            Stepped::Published(_)
        ));
    }
    let Some(RenderRecord::End {
        frames: count,
        tick,
        digest,
        video,
        projection,
        ..
    }) = records.last()
    else {
        panic!("the last record is the end");
    };
    assert_eq!((*count, *tick, video.as_deref()), (3, 6, None));
    assert_eq!(*digest, plain.digest().to_hex());
    // Plan §13.4: the projection's matrix and scaling are recorded, every
    // entry the epochs' own.
    let fixed = flife_render::AffinityProjection::fixed();
    assert_eq!(
        **projection,
        RenderProjection {
            rows: [7, 14, 25],
            matrix: fixed.matrix,
            divisor: 32.0_f64.sqrt(),
            affinity_limit: 1.0,
        }
    );
    let halved = ValidatedConfig::from_toml(&PRESET.replacen(
        "affinity_norm_limit = 1.0",
        "affinity_norm_limit = 0.5",
        1,
    ))
    .expect("a world at half the limit");
    assert_eq!(RenderProjection::of(&halved).affinity_limit, 0.5);

    // Another style draws other pixels of the same world: FL-019's
    // mappings only read epochs.
    let restyled_out = out.with_file_name("flife-render-test-styled");
    std::fs::create_dir_all(&restyled_out).expect("a scratch directory");
    let styled = RenderOptions {
        out: restyled_out,
        style: Style {
            mapping: ColorMapping::Energy,
            look: Look::Raw,
            particles: Particles::Density,
            field: true,
            field_mapping: FieldMapping::Energy,
        },
        ..options
    };
    let mut styled_records = Vec::new();
    render(&config, &styled, |record| {
        styled_records.push(record.clone())
    })
    .expect("renders");
    let Some(RenderRecord::End {
        digest: styled_digest,
        mapping,
        look,
        particles,
        field,
        ..
    }) = styled_records.last()
    else {
        panic!("the last record is the end");
    };
    assert_eq!(*styled_digest, plain.digest().to_hex());
    assert_eq!(
        (*mapping, *look, *particles, *field),
        ("energy", "raw", "density", true)
    );
    let frame = |records: &[RenderRecord], index: u64| -> Vec<u8> {
        let path = records
            .iter()
            .find_map(|record| match record {
                RenderRecord::Frame { frame, path, .. } if *frame == index => Some(path.clone()),
                _ => None,
            })
            .expect("the frame record");
        std::fs::read(path).expect("the frame exists")
    };
    assert_ne!(frame(&records, 2), frame(&styled_records, 2));

    // The field's composition (plan §13.4, "Local field") draws other
    // pixels of the same world, and the end record says which field.
    let composed_out = out.with_file_name("flife-render-test-composition");
    std::fs::create_dir_all(&composed_out).expect("a scratch directory");
    let composed = RenderOptions {
        out: composed_out,
        style: Style {
            field_mapping: FieldMapping::Composition,
            ..Style::default()
        },
        ..styled.clone()
    };
    let mut composed_records = Vec::new();
    render(&config, &composed, |record| {
        composed_records.push(record.clone())
    })
    .expect("renders");
    let field_of = |records: &[RenderRecord]| match records.last() {
        Some(RenderRecord::End {
            digest,
            field_mapping,
            ..
        }) => (digest.clone(), *field_mapping),
        other => panic!("the last record is the end, not {other:?}"),
    };
    assert_eq!(
        field_of(&composed_records),
        (plain.digest().to_hex(), "composition")
    );
    assert_eq!(field_of(&records), (plain.digest().to_hex(), "energy"));
    assert_ne!(frame(&records, 2), frame(&composed_records, 2));

    // A chosen component (plan §13.4, "Local field") draws other pixels
    // again, of the same world, and each channel its own.
    let mut channel_frames = Vec::new();
    for channel in [0, 13] {
        let channel_out = out.with_file_name(format!("flife-render-test-component-{channel}"));
        std::fs::create_dir_all(&channel_out).expect("a scratch directory");
        let component = FieldComponent::new(channel).expect("a channel");
        let options = RenderOptions {
            out: channel_out,
            style: Style {
                field_mapping: FieldMapping::Component(component),
                ..Style::default()
            },
            ..styled.clone()
        };
        let mut channel_records = Vec::new();
        render(&config, &options, |record| {
            channel_records.push(record.clone())
        })
        .expect("renders");
        let (digest, name) = field_of(&channel_records);
        assert_eq!(digest, plain.digest().to_hex());
        assert_eq!(name, format!("component-{channel}"));
        let drawn = frame(&channel_records, 2);
        assert_ne!(drawn, frame(&records, 2));
        assert_ne!(drawn, frame(&composed_records, 2));
        channel_frames.push(drawn);
    }
    assert_ne!(channel_frames[0], channel_frames[1], "each channel its own");

    // The preset blooms glowing frames (FL-019): with `bloom_enabled =
    // false` the glow frame differs, and the raw frame, never bloomed, is
    // the same.
    assert_eq!(PRESET.matches("bloom_enabled = true").count(), 1);
    let unbloomed = ValidatedConfig::from_toml(
        &PRESET.replace("bloom_enabled = true", "bloom_enabled = false"),
    )
    .expect("still valid");
    for (style, bloomed, same) in [
        (Style::default(), &records, false),
        (styled.style, &styled_records, true),
    ] {
        let out = out.with_file_name(format!("flife-render-test-unbloomed-{}", style.look.name()));
        std::fs::create_dir_all(&out).expect("a scratch directory");
        let options = RenderOptions {
            out,
            style,
            ..styled.clone()
        };
        let mut plain = Vec::new();
        render(&unbloomed, &options, |record| plain.push(record.clone())).expect("renders");
        assert_eq!(
            frame(&plain, 2) == frame(bloomed, 2),
            same,
            "{}",
            style.look.name()
        );
    }

    // The preset's dots are at least `point_radius_pixels` (0.8 points)
    // on screen (FL-019). At 96 x 64 the world is 0.5 px a unit, so the
    // floor binds: the old fixed 1.5 points draws other pixels in both
    // looks, and density, which draws no dots, the same.
    assert_eq!(PRESET.matches("point_radius_pixels = 0.8").count(), 1);
    let coarse = ValidatedConfig::from_toml(
        &PRESET.replace("point_radius_pixels = 0.8", "point_radius_pixels = 1.5"),
    )
    .expect("still valid");
    let raw_points = Style {
        look: Look::Raw,
        ..Style::default()
    };
    for (style, same) in [
        (Style::default(), false),
        (raw_points, false),
        (styled.style, true),
    ] {
        let radius_frame = |config: &ValidatedConfig, name: &str| {
            let out = out.with_file_name(format!(
                "flife-render-test-radius-{name}-{}-{}",
                style.look.name(),
                style.particles.name()
            ));
            std::fs::create_dir_all(&out).expect("a scratch directory");
            let options = RenderOptions {
                out,
                style,
                ..styled.clone()
            };
            let mut records = Vec::new();
            render(config, &options, |record| records.push(record.clone())).expect("renders");
            frame(&records, 2)
        };
        assert_eq!(
            radius_frame(&config, "fine") == radius_frame(&coarse, "coarse"),
            same,
            "{} {}",
            style.look.name(),
            style.particles.name()
        );
    }
}

/// A camera path (FL-020's pan and zoom) changes which part of the torus
/// each frame shows and nothing the world computes: the same digest as the
/// whole-torus render, each frame record's view where the path puts it, and
/// other pixels once it has zoomed.
#[test]
fn a_camera_path_moves_the_view_and_leaves_the_world_alone() {
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let [width, height] = [config.config().world.width, config.config().world.height];
    let scratch = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let options = |name: &str, camera: Option<CameraPath>| {
        let out = scratch.join(name);
        std::fs::create_dir_all(&out).expect("a scratch directory");
        RenderOptions {
            ticks: 8,
            every: 4,
            size: [96, 64],
            out,
            cpu_workers: NonZeroUsize::new(2),
            controllers: ControllerChoice::Cpu,
            render_threads: NonZeroUsize::MIN,
            video: None,
            fps: 30,
            style: Style::default(),
            camera,
        }
    };
    // Hold the whole torus to tick 4, then ease to zoom 4 on a point near
    // the corner by tick 8, crossing the seam.
    let camera = CameraPath::from_json(&format!(
        r#"{{"keys": [{{"tick": 4, "center": [{}, {}], "zoom": 1}},
                     {{"tick": 8, "center": [{}, 2.5], "zoom": 4}}]}}"#,
        width / 2.0,
        height / 2.0,
        width - 1.5,
    ))
    .expect("a path");
    let mut plain = Vec::new();
    render(&config, &options("flife-render-camera-plain", None), |r| {
        plain.push(r.clone());
    })
    .expect("renders");
    let mut moved = Vec::new();
    render(
        &config,
        &options("flife-render-camera-moved", Some(camera.clone())),
        |r| moved.push(r.clone()),
    )
    .expect("renders");

    let end = |records: &[RenderRecord]| match records.last() {
        Some(RenderRecord::End { digest, camera, .. }) => (digest.clone(), camera.clone()),
        _ => panic!("the last record is the end"),
    };
    assert_eq!(end(&plain), (end(&moved).0, None));
    assert_eq!(end(&moved).1, Some(camera));

    let frames = |records: &[RenderRecord]| -> Vec<(u64, CameraView, Vec<u8>)> {
        records
            .iter()
            .filter_map(|record| match record {
                RenderRecord::Frame {
                    tick, view, path, ..
                } => Some((*tick, *view, std::fs::read(path).expect("the frame"))),
                _ => None,
            })
            .collect()
    };
    let (plain, moved) = (frames(&plain), frames(&moved));
    assert_eq!(plain.len(), 3);
    let whole = CameraView {
        center: [width / 2.0, height / 2.0],
        zoom: 1.0,
    };
    for (index, ((tick, view, pixels), (moved_tick, moved_view, moved_pixels))) in
        plain.iter().zip(&moved).enumerate()
    {
        assert_eq!(tick, moved_tick);
        assert_eq!(
            *view, whole,
            "without a camera every frame is the whole torus"
        );
        if index < 2 {
            // Ticks 0 and 4: held at the first key, the whole torus.
            assert_eq!(*moved_view, whole);
            assert_eq!(pixels, moved_pixels, "tick {tick}: the same pixels");
        } else {
            assert_eq!(
                *moved_view,
                CameraView {
                    center: [width - 1.5, 2.5],
                    zoom: 4.0
                }
            );
            assert_ne!(pixels, moved_pixels, "tick {tick}: the zoomed view");
        }
    }
}

/// The `flife render` binary colours by the configuration's
/// `[presentation] color_mapping` unless `--color` names another (in either
/// spelling); a configuration naming an unregistered mapping is refused
/// before anything renders.
#[test]
fn the_render_colour_is_the_configured_mapping_unless_the_flag_overrides_it() {
    let scratch = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let configured = |mapping: &str| {
        let path = scratch.join(format!("fl019-{mapping}.toml"));
        let text = PRESET.replace(
            "color_mapping = \"founder-lineage-v1\"",
            &format!("color_mapping = \"{mapping}\""),
        );
        assert_ne!(text, PRESET, "the preset names its colour mapping");
        std::fs::write(&path, text).expect("write the configuration");
        path
    };
    let render = |config: &std::path::Path, extra: &[&str]| {
        let out = scratch.join("fl019-render-flags");
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_flife"))
            .arg("render")
            .arg(config)
            .args([
                "--ticks", "1", "--every", "1", "--width", "32", "--height", "32",
            ])
            .arg("--out")
            .arg(&out)
            .args(extra)
            .output()
            .expect("flife runs");
        let last = |bytes: &[u8]| -> Option<serde_json::Value> {
            String::from_utf8_lossy(bytes)
                .lines()
                .last()
                .map(|line| serde_json::from_str(line).expect("one JSON record per line"))
        };
        (
            output.status.code(),
            last(&output.stdout),
            last(&output.stderr),
        )
    };

    let energy = configured("energy-lightness-v1");
    let (status, end, err) = render(&energy, &[]);
    assert_eq!(status, Some(0), "{err:?}");
    let end = end.expect("an end record");
    assert_eq!(end["record"], "end");
    assert_eq!(end["mapping"], "energy", "the configured mapping");
    assert_eq!(
        end["field_mapping"], "energy",
        "the field's energy by default"
    );

    // FL-019: the flag draws the field's composition instead.
    let (status, end, err) = render(&energy, &["--field-mapping", "composition"]);
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(
        end.expect("an end record")["field_mapping"],
        "composition",
        "--field-mapping composition"
    );
    let (status, end, err) = render(&energy, &["--field-mapping", "component-31"]);
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(
        end.expect("an end record")["field_mapping"],
        "component-31",
        "--field-mapping component-31"
    );
    // A channel past the last is a usage error, which clap reports in text.
    let refused = std::process::Command::new(env!("CARGO_BIN_EXE_flife"))
        .arg("render")
        .arg(&energy)
        .args(["--ticks", "1", "--field-mapping", "component-32", "--out"])
        .arg(scratch.join("fl019-render-refused"))
        .output()
        .expect("flife runs");
    assert_eq!(refused.status.code(), Some(2));
    assert!(refused.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("`component-32` is not a field mapping"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );

    for flag in ["motion", "motion-heading-v1"] {
        let (status, end, err) = render(&energy, &["--color", flag]);
        assert_eq!(status, Some(0), "{err:?}");
        assert_eq!(
            end.expect("an end record")["mapping"],
            "motion",
            "--color {flag}"
        );
    }

    let (status, out, err) = render(&configured("glitter-v1"), &[]);
    assert_eq!(status, Some(2));
    assert_eq!(out, None, "nothing rendered");
    let err = err.expect("an error record");
    assert_eq!(err["kind"], "invalid-config");
    assert!(
        err["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("glitter-v1")),
        "{err}"
    );
}

/// `flife render --video` with this machine's ffmpeg: an H.264 MP4 of the
/// frames, converted as the Studio export's y4m is (fmn-frame's
/// `rgba_to_nv12`: BT.709, limited range, left siting) and tagged so. Each
/// frame decodes to that conversion of its PNG within compression. Without
/// an ffmpeg listing libx264 nothing here runs; `tests/render_run.rs`
/// refuses a video with no ffmpeg on the `PATH`.
#[test]
fn a_video_holds_the_frames_in_bt709_limited_range_and_says_so() {
    if let Err(error) = flife_cli::video_encoder() {
        println!("no H.264 encoder ({error}): skipped");
        return;
    }
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("after the epoch")
        .as_nanos();
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("flife-render-video-{}-{nanos}", std::process::id()));
    let out = dir.join("frames");
    std::fs::create_dir_all(&out).expect("a new directory");
    let video = dir.join("render.mp4");
    let [width, height] = [192_u32, 128];
    let options = RenderOptions {
        ticks: 6,
        every: 3,
        size: [width, height],
        out,
        cpu_workers: NonZeroUsize::new(3),
        controllers: ControllerChoice::Auto,
        render_threads: NonZeroUsize::MIN,
        video: Some(video.clone()),
        fps: 30,
        style: Style::default(),
        camera: None,
    };
    let mut records = Vec::new();
    render(&config, &options, |record| records.push(record.clone())).expect("renders");
    let paths: Vec<String> = records
        .iter()
        .filter_map(|record| match record {
            RenderRecord::Frame { path, .. } => Some(path.clone()),
            RenderRecord::Start { .. } | RenderRecord::End { .. } => None,
        })
        .collect();
    assert_eq!(paths.len(), 3);

    // The stream: H.264 4:2:0 at 30/1, a frame a PNG, tagged as converted.
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,pix_fmt,width,height,r_frame_rate,nb_frames,color_range,\
             color_space,color_primaries,color_transfer,chroma_location",
            "-of",
            "json",
        ])
        .arg(&video)
        .output()
        .expect("ffprobe runs beside ffmpeg");
    assert!(probe.status.success(), "{probe:?}");
    let probed: serde_json::Value = serde_json::from_slice(&probe.stdout).expect("ffprobe's JSON");
    let stream = &probed["streams"][0];
    for (name, expected) in [
        ("codec_name", "h264"),
        ("pix_fmt", "yuv420p"),
        ("r_frame_rate", "30/1"),
        ("nb_frames", "3"),
        ("color_range", "tv"),
        ("color_space", "bt709"),
        ("color_primaries", "bt709"),
        ("color_transfer", "iec61966-2-1"),
        ("chroma_location", "left"),
    ] {
        assert_eq!(stream[name].as_str(), Some(expected), "{name}: {stream}");
    }
    assert_eq!(
        [stream["width"].as_u64(), stream["height"].as_u64()],
        [Some(u64::from(width)), Some(u64::from(height))]
    );

    // Decoded: each frame is its PNG converted by `rgba_to_nv12`, within
    // compression. Which matrix and siting converted it are told apart from
    // the compression's loss by projection. The decoded luma's departure
    // from that conversion is projected onto how BT.601's luma departs from
    // BT.709's: about 0 for BT.709, about 1 for BT.601. Left to itself,
    // ffmpeg 8.1 made it 1.004 on these frames at 34.9 dB; converted as now,
    // 0.020 at 38.8 dB. The chroma's is projected onto how centre-sited
    // chroma departs from left-sited.
    let decoded = std::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-i"])
        .arg(&video)
        .args(["-f", "rawvideo", "-"])
        .output()
        .expect("ffmpeg decodes");
    assert!(decoded.status.success(), "{decoded:?}");
    let luma = width as usize * height as usize;
    let frame_bytes = luma * 3 / 2;
    assert_eq!(decoded.stdout.len(), frame_bytes * paths.len());
    let mut worst = (f64::INFINITY, f64::INFINITY);
    // Departures projected onto BT.601's luma and onto centre-sited chroma.
    let mut matrix = Leaning::default();
    let mut siting = Leaning::default();
    for (index, (path, theirs)) in paths
        .iter()
        .zip(decoded.stdout.chunks_exact(frame_bytes))
        .enumerate()
    {
        let png = decode(
            &std::fs::read(path).expect("the frame"),
            &PngLimits::default(),
        )
        .expect("decodes");
        let mut rgba = FrameBuffer::new(
            FrameLayout::tight(PixelFormat::Rgba8, width, height).expect("a layout"),
        );
        rgba.as_bytes_mut().copy_from_slice(&png.rgba);
        let ours = i420(&rgba, ChromaSiting::Left);
        let centred = i420(&rgba, ChromaSiting::Center);
        assert_eq!(ours.len(), frame_bytes);
        let (luma_psnr, chroma_psnr) = (
            psnr(&ours[..luma], &theirs[..luma]),
            psnr(&ours[luma..], &theirs[luma..]),
        );
        worst = (worst.0.min(luma_psnr), worst.1.min(chroma_psnr));
        assert!(
            luma_psnr >= 36.0 && chroma_psnr >= 31.0,
            "frame {index}: luma PSNR {luma_psnr:.1} dB, chroma {chroma_psnr:.1} dB"
        );
        for ((pixel, &converted), &shown) in png
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(&ours[..luma])
            .zip(&theirs[..luma])
        {
            let bt601 = studio_luma(pixel, 0.299, 0.114) - studio_luma(pixel, 0.2126, 0.0722);
            matrix.add(f64::from(shown) - f64::from(converted), bt601);
        }
        for ((&left, &centre), &shown) in ours[luma..]
            .iter()
            .zip(&centred[luma..])
            .zip(&theirs[luma..])
        {
            siting.add(
                f64::from(shown) - f64::from(left),
                f64::from(centre) - f64::from(left),
            );
        }
    }
    let (toward_601, toward_centre) = (matrix.leaning(), siting.leaning());
    println!(
        "worst luma PSNR {:.1} dB, chroma {:.1} dB; toward BT.601 {toward_601:.3}, toward centre \
         siting {toward_centre:.3}",
        worst.0, worst.1
    );
    assert!(
        toward_601.abs() <= 0.25,
        "the luma leans {toward_601:.3} toward BT.601"
    );
    // ffmpeg's left-sited chroma filter is wider than `rgba_to_nv12`'s
    // two-tap one. Left-sited, it read 0.70 toward the centre-sited box on
    // the first frame unencoded (0.725 here); centre-sited, 0.999.
    assert!(
        toward_centre <= 0.85,
        "the chroma leans {toward_centre:.3} toward centre siting"
    );
}

/// `rgba` converted as the Studio export converts it (`rgba_to_nv12`,
/// limited range) with chroma `siting`: the I420 planes, Y then Cb and Cr,
/// of its y4m record.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure is a test failure"
)]
fn i420(rgba: &FrameBuffer, siting: ChromaSiting) -> Vec<u8> {
    let layout = rgba.layout();
    let (width, height) = (layout.width(), layout.height());
    let mut nv12 =
        FrameBuffer::new(FrameLayout::tight(PixelFormat::Nv12, width, height).expect("even"));
    rgba_to_nv12(rgba, &mut nv12, ColorRange::Limited, siting).expect("converts");
    let mut record = Vec::new();
    append_y4m_frame_nv12(&mut record, width, height, &nv12).expect("a record");
    record.split_off(6)
}

/// How far a decoded frame's departures from a reference lean toward
/// another reference: the departures projected onto the other's
/// difference from the reference, about 0 when it is the reference and 1
/// when it is the other.
#[derive(Default)]
struct Leaning {
    along: f64,
    apart: f64,
}

impl Leaning {
    fn add(&mut self, departure: f64, toward: f64) {
        self.along += departure * toward;
        self.apart += toward * toward;
    }

    fn leaning(&self) -> f64 {
        assert!(self.apart > 0.0, "the references differ");
        self.along / self.apart
    }
}

/// The PSNR of samples `theirs` against `ours` in dB.
fn psnr(ours: &[u8], theirs: &[u8]) -> f64 {
    assert_eq!(ours.len(), theirs.len());
    let squared: f64 = ours
        .iter()
        .zip(theirs)
        .map(|(&a, &b)| (f64::from(b) - f64::from(a)).powi(2))
        .sum();
    10.0 * (255.0 * 255.0 / (squared / ours.len() as f64).max(1e-9)).log10()
}

/// The limited-range luma of an sRGB-encoded RGBA pixel under the matrix
/// with red and blue weights `kr` and `kb`.
fn studio_luma(pixel: &[u8; 4], kr: f64, kb: f64) -> f64 {
    let [r, g, b, _] = pixel.map(f64::from);
    16.0 + 219.0 * (kr * r + (1.0 - kr - kb) * g + kb * b) / 255.0
}
