//! `flife render`: pre-rendered frames of the reference world through the
//! Studio's FrankenManim view, without perturbing the world.

use std::num::NonZeroUsize;

use flife_cli::{
    ColorMapping, ControllerChoice, Look, Particles, RENDER_SCHEMA, RenderOptions, RenderRecord,
    Style, render,
};
use flife_core::ValidatedConfig;
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};

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
        ..
    }) = records.last()
    else {
        panic!("the last record is the end");
    };
    assert_eq!((*count, *tick, video.as_deref()), (3, 6, None));
    assert_eq!(*digest, plain.digest().to_hex());

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
