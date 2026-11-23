//! FL-050 / FL-043 / G10 / V2-16 Cinema export qualification and regression
//! tests (plan §13.8, §19.4, §21.3).
//!
//! Pure-CPU tests (1080p/4K peak-memory admission, integer-tick
//! simulation-time sampling, torus-aware camera timeline stability, pre-write
//! timeline validation, V2-16 style gating, and manifest backward
//! compatibility) run on every host. End-to-end 1080p and 4K rendered G10
//! tests run on Metal hosts and skip cleanly when no Metal device is present.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "integration tests: an unexpected failure is a test failure"
)]

use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flife_core::{CoreError, FeatureOverride, Sha256Digest, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, World};
use flife_manim::{MAXIMUM_ZOOM, Viewport, WorldView};
use flife_render::Epoch;
use flife_studio::memory::{admit_export, export_scratch_bytes, video_scratch_bytes};
use flife_studio::{
    CINEMA_4K, CINEMA_1080P, CameraKey, CameraPath, CinemaOptions, ColorMapping, ControllerChoice,
    EXPORT_SCHEMA, Exact, ExportManifest, ExportMode, FieldMapping, KEY_8, KEY_9, KEY_C, KEY_E,
    KEY_PERIOD, Look, MANIFEST_FILE, REPLAY_SLOTS, Status, Studio, StudioOptions, Style, Surface,
    TimelineCheckpoint, TimelineRecord, TimelineStart, TimelineStyle, VIDEO_FILE, export_cinema,
    parse_cinema_resolution, peak_observer_bytes_for, plan_simulation_samples, read_timeline,
    validate_timeline, validate_v2_style, write_timeline_record,
};
use fmn_codec::{PngLimits, decode_png, decode_y4m};
use fmn_render::metal::{InputEvent, LogicalPoint, MetalRenderer, Modifiers, PointerButton};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

fn fresh(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("after epoch")
        .as_nanos();
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("fl050-{name}-{}-{nanos}", std::process::id()))
}

fn preset_with(pairs: &[(&str, &str)], model: &str, with_contact: bool) -> String {
    let mut lines: Vec<String> = PRESET.lines().map(str::to_owned).collect();
    for &(key, value) in pairs {
        let line = lines
            .iter()
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .unwrap_or_else(|| panic!("{key} is in the preset"));
        lines[line] = format!("{key} = {value}");
    }
    let mut text = lines.join("\n").replacen(
        "model = \"field-ecology-v1\"",
        &format!("model = \"{model}\""),
        1,
    );
    if with_contact {
        text.push_str(
            "\n[contact]\ndiameter = 0.25\nstiffness = 0.5\nadhesion = 0.25\nadhesion_range = 2.0\n",
        );
    }
    text
}

fn found(config: &ValidatedConfig) -> World {
    World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds")
}

fn timeline_text(records: &[TimelineRecord]) -> String {
    let mut out = Vec::new();
    for record in records {
        write_timeline_record(&mut out, record).expect("serializes timeline record");
    }
    String::from_utf8(out).expect("utf-8")
}

/// FL-043 / FL-050 (`INV-15`): 1080p and 4K Cinema export admission honestly
/// accounts for replay observer buffers, PNG encoder working set, NV12 + y4m
/// scratch, and V2-16 optional epoch deltas — and refuses tight budgets with
/// `CoreError::BudgetExhausted`.
#[test]
fn fl043_cinema_1080p_and_4k_export_admission_accounts_buffers_and_refuses_tight_budgets() {
    let base_toml = preset_with(
        &[("maximum_total_bytes", "2147483648")],
        "field-ecology-v1",
        false,
    );
    let config = ValidatedConfig::from_toml(&base_toml).expect("valid v1 config");
    let world = found(&config);
    let founding = Epoch::standalone(&world).expect("epoch");
    let mut view = WorldView::for_epoch(&founding, [320, 200], [0, 0, 0]).expect("view");
    view.enable_overlay();
    let header = TimelineRecord::header(&config, [320, 200], Style::default());
    let records = [
        header,
        TimelineRecord::End {
            tick: 0,
            digest: world.digest().to_hex(),
            applied: Vec::new(),
        },
    ];

    // Check exact byte accounting at 1080p (1x) and 4K (2x), with and without y4m video.
    for (size, scale) in [(CINEMA_1080P, 1.0), (CINEMA_4K, 2.0)] {
        let peak_obs = peak_observer_bytes_for(
            &world,
            &founding,
            &view,
            [320, 200],
            &records,
            Some((size, scale)),
        )
        .expect("peak observers");
        let with_video = admit_export(
            &world,
            &founding,
            &view,
            [320, 200],
            &records,
            Some((size, scale)),
            true,
        )
        .expect("admits with video");
        let without_video = admit_export(
            &world,
            &founding,
            &view,
            [320, 200],
            &records,
            Some((size, scale)),
            false,
        )
        .expect("admits without video");
        assert_eq!(
            with_video.render,
            peak_obs + export_scratch_bytes(size, true)
        );
        assert_eq!(
            without_video.render,
            peak_obs + export_scratch_bytes(size, false)
        );
        assert_eq!(
            with_video.render - without_video.render,
            video_scratch_bytes(size)
        );
        assert!(
            with_video.render > peak_obs,
            "export encoder scratch must be charged on top of observer bytes"
        );

        // 1 byte short of admitted peak must refuse cleanly with BudgetExhausted.
        let tight_budget = (with_video.peak_bytes - 1).to_string();
        let tight_toml = preset_with(
            &[("maximum_total_bytes", &tight_budget)],
            "field-ecology-v1",
            false,
        );
        let tight_cfg = ValidatedConfig::from_toml(&tight_toml).expect("valid tight config");
        let tight_world = found(&tight_cfg);
        let tight_epoch = Epoch::standalone(&tight_world).expect("epoch");
        let mut tight_view =
            WorldView::for_epoch(&tight_epoch, [320, 200], [0, 0, 0]).expect("view");
        tight_view.enable_overlay();
        let err = admit_export(
            &tight_world,
            &tight_epoch,
            &tight_view,
            [320, 200],
            &records,
            Some((size, scale)),
            true,
        )
        .expect_err("budget 1 byte short must refuse");
        assert!(
            matches!(err, CoreError::BudgetExhausted { .. }),
            "expected BudgetExhausted at {size:?}, got {err:?}"
        );
    }

    // V2-16 optional Epoch buffers scale honestly by REPLAY_SLOTS in 4K export admission.
    let v2_toml = preset_with(
        &[("maximum_total_bytes", "2147483648")],
        "field-ecology-v2",
        true,
    );
    let v2_on_cfg = ValidatedConfig::from_toml_with_overrides(
        &v2_toml,
        &[FeatureOverride::Enable("all-v2".to_owned())],
    )
    .expect("v2 on");
    let v2_off_cfg = ValidatedConfig::from_toml_with_overrides(
        &v2_toml,
        &[
            FeatureOverride::Enable("all-v2".to_owned()),
            FeatureOverride::Disable("v2-visual-mappings".to_owned()),
        ],
    )
    .expect("v2 off");
    let w_on = found(&v2_on_cfg);
    let w_off = found(&v2_off_cfg);
    let e_on = Epoch::standalone(&w_on).expect("epoch on");
    let e_off = Epoch::standalone(&w_off).expect("epoch off");
    let mut v_on = WorldView::for_epoch(&e_on, [320, 200], [0, 0, 0]).expect("view on");
    let mut v_off = WorldView::for_epoch(&e_off, [320, 200], [0, 0, 0]).expect("view off");
    v_on.enable_overlay();
    v_off.enable_overlay();
    let a_on = admit_export(
        &w_on,
        &e_on,
        &v_on,
        [320, 200],
        &[],
        Some((CINEMA_4K, 2.0)),
        true,
    )
    .expect("v2 on 4K admits");
    let a_off = admit_export(
        &w_off,
        &e_off,
        &v_off,
        [320, 200],
        &[],
        Some((CINEMA_4K, 2.0)),
        true,
    )
    .expect("v2 off 4K admits");
    let expected_delta = (e_on.reserved_bytes() - e_off.reserved_bytes()) * REPLAY_SLOTS as u64;
    assert!(expected_delta > 0);
    assert_eq!(a_on.render - a_off.render, expected_delta);
}

/// FL-050 (G10): exact integer-tick simulation-time sampling has zero
/// floating-point clock drift, and torus-aware smoothstep camera paths are
/// strictly deterministic and resolution-independent across 1080p and 4K.
#[test]
fn fl050_simulation_time_sampling_and_camera_timeline_stability() {
    let fps = NonZeroU32::new(60).expect("fps");
    let every = NonZeroU64::new(4).expect("every");
    let dt = 1.0 / 128.0;

    // Exact integer sampling from resumed tick 128 through 140: 128, 132, 136, 140.
    let samples = plan_simulation_samples(128, 140, every, dt, fps).expect("samples");
    assert_eq!(samples.len(), 4);
    for (idx, sample) in samples.iter().enumerate() {
        let expected_tick = 128 + idx as u64 * 4;
        assert_eq!(sample.frame, idx as u64);
        assert_eq!(sample.tick, expected_tick);
        assert_eq!(sample.sim_seconds, expected_tick as f64 * dt);
        assert_eq!(sample.video_seconds, idx as f64 / 60.0);
    }

    // Long horizon: 10,000 ticks at step 10 stays exact with zero cumulative drift.
    let long = plan_simulation_samples(0, 100_000, NonZeroU64::new(10).expect("10"), dt, fps)
        .expect("long samples");
    assert_eq!(long.len(), 10_001);
    assert_eq!(long.last().expect("last").tick, 100_000);
    assert_eq!(long.last().expect("last").sim_seconds, 100_000.0 * dt);

    // Invalid ranges / timesteps refused cleanly.
    assert!(plan_simulation_samples(10, 5, every, dt, fps).is_err());
    assert!(plan_simulation_samples(0, 10, every, 0.0, fps).is_err());
    assert!(plan_simulation_samples(0, 10, every, f64::NAN, fps).is_err());

    // Resolution parsing & CinemaOptions target resolution defaults.
    assert_eq!(
        parse_cinema_resolution("1080p").expect("1080p"),
        CINEMA_1080P
    );
    assert_eq!(parse_cinema_resolution("4K").expect("4K"), CINEMA_4K);
    assert_eq!(parse_cinema_resolution("2160p").expect("2160p"), CINEMA_4K);
    assert_eq!(
        parse_cinema_resolution("1280x720").expect("720p"),
        [1280, 720]
    );
    for bad in ["0x1080", "1920x0", "720p", "abc", "1920"] {
        assert!(parse_cinema_resolution(bad).is_err(), "{bad}");
    }
    let opts_1080_preset = CinemaOptions::cinema_1080p(fps, every);
    assert_eq!(
        opts_1080_preset.mode,
        ExportMode::SimulationTime { every_ticks: every }
    );
    assert_eq!(
        opts_1080_preset.resolved_target().expect("1080p target"),
        Some((CINEMA_1080P, 1.0))
    );
    let opts_4k_preset = CinemaOptions::cinema_4k(fps, every);
    assert_eq!(
        opts_4k_preset.mode,
        ExportMode::SimulationTime { every_ticks: every }
    );
    assert_eq!(
        opts_4k_preset.resolved_target().expect("4K target"),
        Some((CINEMA_4K, 2.0))
    );

    // Camera timeline stability across a torus seam and across 1080p vs 4K viewports.
    let extent = [256.0, 256.0];
    let camera = CameraPath {
        keys: vec![
            CameraKey {
                tick: 0,
                center: [250.0, 128.0],
                zoom: 1.0,
            },
            CameraKey {
                tick: 100,
                center: [6.0, 128.0],
                zoom: 4.0,
            },
        ],
    };
    let vp_1080 = Viewport::fit(extent, CINEMA_1080P).expect("1080p vp");
    let vp_4k = Viewport::fit(extent, CINEMA_4K).expect("4K vp");
    let mut prev_zoom = 0.0_f64;
    for tick in (0..=100).step_by(5) {
        let (v1, c1) = camera.viewport(&vp_1080, tick).expect("1080p cam");
        let (v2, c2) = camera.viewport(&vp_4k, tick).expect("4K cam");
        assert_eq!(
            c1, c2,
            "camera view at tick {tick} must not depend on resolution"
        );
        assert_eq!(v1.center(), v2.center());
        assert_eq!(v1.zoom().to_bits(), v2.zoom().to_bits());
        assert!(c1.zoom >= prev_zoom && c1.zoom <= MAXIMUM_ZOOM);
        prev_zoom = c1.zoom;
    }
    // Midpoint crosses seam cleanly at x = 2.0 (250 + 6 = 256 -> 0, +2 -> 2.0) and geometric zoom = 2.0.
    let mid = camera.at(50, extent);
    assert!((mid.center[0] - 2.0).abs() < 1e-12, "mid = {mid:?}");
    assert!((mid.zoom - 2.0).abs() < 1e-12, "mid = {mid:?}");
}

/// G10: `validate_timeline` and `export_cinema` reject corrupted timelines and
/// disallowed `V2-16` visual styles BEFORE creating `out` on disk.
#[test]
fn g10_validate_timeline_catches_corruption_and_enforces_v2_visual_gate_before_writing() {
    let v1_cfg = ValidatedConfig::from_toml(PRESET).expect("valid v1");
    let v2_toml = preset_with(&[], "field-ecology-v2", true);
    let v2_on = ValidatedConfig::from_toml_with_overrides(
        &v2_toml,
        &[FeatureOverride::Enable("all-v2".to_owned())],
    )
    .expect("v2 on");
    let v2_off = ValidatedConfig::from_toml_with_overrides(
        &v2_toml,
        &[
            FeatureOverride::Enable("all-v2".to_owned()),
            FeatureOverride::Disable("v2-visual-mappings".to_owned()),
        ],
    )
    .expect("v2 off");

    let end = |tick| TimelineRecord::End {
        tick,
        digest: "00".repeat(32),
        applied: Vec::new(),
    };
    let v1_header = TimelineRecord::header(&v1_cfg, [320, 200], Style::default());

    // 1. Structural & monotonicity refusals.
    let bad_cases: [(&str, Vec<TimelineRecord>); 8] = [
        (
            "duplicate header",
            vec![v1_header.clone(), v1_header.clone(), end(0)],
        ),
        ("early end", vec![v1_header.clone(), end(0), end(0)]),
        (
            "non-sequential frame index",
            vec![
                v1_header.clone(),
                TimelineRecord::Draw {
                    frame: 1,
                    tick: 0,
                    wall_seconds: Exact(0.0),
                },
                end(0),
            ],
        ),
        (
            "draw tick past end",
            vec![
                v1_header.clone(),
                TimelineRecord::Show {
                    prior: None,
                    current: 5,
                    fraction: Exact(1.0),
                },
                TimelineRecord::Draw {
                    frame: 0,
                    tick: 5,
                    wall_seconds: Exact(0.0),
                },
                end(3),
            ],
        ),
        (
            "prior after current",
            vec![
                v1_header.clone(),
                TimelineRecord::Show {
                    prior: Some(4),
                    current: 2,
                    fraction: Exact(0.5),
                },
                end(4),
            ],
        ),
        (
            "fraction out of range",
            vec![
                v1_header.clone(),
                TimelineRecord::Show {
                    prior: None,
                    current: 1,
                    fraction: Exact(1.25),
                },
                end(2),
            ],
        ),
        (
            "negative wall seconds",
            vec![
                v1_header.clone(),
                TimelineRecord::Draw {
                    frame: 0,
                    tick: 0,
                    wall_seconds: Exact(-0.01),
                },
                end(0),
            ],
        ),
        (
            "zero resize drawable",
            vec![
                v1_header.clone(),
                TimelineRecord::Resize { drawable: [0, 200] },
                end(0),
            ],
        ),
    ];
    for (label, records) in bad_cases {
        assert!(
            validate_timeline(&v1_cfg, &records).is_err(),
            "{label} must be refused"
        );
        let out = fresh("refused-corrupt");
        let text = timeline_text(&records);
        let opts = CinemaOptions::cinema_1080p(
            NonZeroU32::new(30).expect("fps"),
            NonZeroU64::new(1).expect("1"),
        );
        assert!(
            export_cinema(&v1_cfg, &text, &opts, &out).is_err(),
            "{label} must fail export"
        );
        assert!(!out.exists(), "{label} must not create output directory");
    }

    // 2. V2-16 visual style gate: refused on v1 and v2 --disable v2-visual-mappings,
    //    accepted on v2 with v2-visual-mappings enabled.
    let v2_styles = [
        Style {
            mapping: ColorMapping::CellState,
            ..Style::default()
        },
        Style {
            mapping: ColorMapping::Polarity,
            ..Style::default()
        },
        Style {
            field_mapping: FieldMapping::TrailCarrier,
            ..Style::default()
        },
        Style {
            field_mapping: FieldMapping::VascularConductance,
            ..Style::default()
        },
    ];
    for style in v2_styles {
        validate_v2_style(&v2_on, style).expect("allowed when v2-visual-mappings enabled");
        assert!(matches!(
            validate_v2_style(&v1_cfg, style),
            Err(CoreError::StateConflict { .. })
        ));
        assert!(matches!(
            validate_v2_style(&v2_off, style),
            Err(CoreError::StateConflict { .. })
        ));

        let v1_with_v2_style = vec![
            v1_header.clone(),
            TimelineRecord::Style {
                style: style.into(),
            },
            end(0),
        ];
        assert!(matches!(
            validate_timeline(&v1_cfg, &v1_with_v2_style),
            Err(CoreError::StateConflict { .. })
        ));

        let v2_off_with_style = vec![
            TimelineRecord::header(&v2_off, [320, 200], Style::default()),
            TimelineRecord::Style {
                style: style.into(),
            },
            end(0),
        ];
        assert!(matches!(
            validate_timeline(&v2_off, &v2_off_with_style),
            Err(CoreError::StateConflict { .. })
        ));

        let v2_on_with_style = vec![
            TimelineRecord::header(&v2_on, [320, 200], style),
            TimelineRecord::Draw {
                frame: 0,
                tick: 0,
                wall_seconds: Exact(0.0),
            },
            end(0),
        ];
        let validated =
            validate_timeline(&v2_on, &v2_on_with_style).expect("v2 style validates when enabled");
        assert_eq!(validated.initial_style, style);
        assert_eq!(validated.draws[0].style, style);
    }

    // 3. Invalid CinemaOptions resolution / scale refused before touching disk.
    let valid_text = timeline_text(&[v1_header, end(0)]);
    let mut bad_opts = CinemaOptions::cinema_1080p(
        NonZeroU32::new(30).expect("fps"),
        NonZeroU64::new(1).expect("1"),
    );
    bad_opts.resolution = Some([0, 1080]);
    let out = fresh("bad-res");
    assert!(matches!(
        export_cinema(&v1_cfg, &valid_text, &bad_opts, &out),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(!out.exists());
}

/// Backward compatibility: legacy v1 `ExportManifest` JSON without FL-050 /
/// V2-16 optional fields deserializes cleanly, and enriched manifests round-trip.
#[test]
fn export_manifest_v2_provenance_round_trips_and_stays_v1_backward_compatible() {
    let legacy = serde_json::json!({
        "schema": EXPORT_SCHEMA,
        "mode": "live-capture",
        "timeline_schema": "frankenlife.timeline.v1",
        "timeline_sha256": "aa",
        "config_hash": "bb",
        "name": "legacy",
        "seed": 42,
        "ticks": [0, 10],
        "end_tick": 10,
        "seconds_per_tick": 0.0078125,
        "digest": "cc",
        "commands": 0,
        "renderer": "fmn-render",
        "tone_mapping": "srgb",
        "projection": flife_studio::TimelineProjection::from(flife_render::AffinityProjection::fixed()),
        "frame_encoder": "fmn-codec png, fast",
        "wall_seconds": 0.5,
        "ticks_per_wall_second": 20.0,
        "frames": [{
            "frame": 0,
            "tick": 0,
            "wall_seconds": 0.0,
            "file": "frame-000000.png",
            "sha256": "dd",
            "slots": 1
        }],
        "video": null,
        "video_withheld": "none"
    });
    let parsed: ExportManifest = serde_json::from_value(legacy).expect("legacy v1 manifest parses");
    assert_eq!(parsed.model, None);
    assert_eq!(parsed.v2_features, None);
    assert_eq!(parsed.v2_visual_mappings, None);
    assert_eq!(parsed.from, None);
    assert_eq!(parsed.sample_every_ticks, None);
    assert_eq!(parsed.camera_keys, None);
    assert!(parsed.color_mappings.is_empty());
    assert!(parsed.field_mappings.is_empty());
    assert_eq!(parsed.admitted_peak_bytes, None);
    assert_eq!(parsed.frames[0].sim_seconds, None);
    assert_eq!(parsed.frames[0].mapping, None);
    assert_eq!(parsed.frames[0].field_mapping, None);

    // Enriched v2 Cinema manifest round-trips cleanly.
    let mut enriched = parsed;
    enriched.mode = "simulation-time".to_owned();
    enriched.model = Some("field-ecology-v2".to_owned());
    enriched.v2_features = Some(vec![
        "epigenetic-recurrence".to_owned(),
        "pcp-polarity".to_owned(),
    ]);
    enriched.v2_visual_mappings = Some(true);
    enriched.from = Some(TimelineStart {
        run_dir: "/tmp/run".to_owned(),
        checkpoint: Some(TimelineCheckpoint {
            sequence: 2,
            tick: 10,
            pin: "deadbeef".to_owned(),
        }),
        tick: 10,
    });
    enriched.sample_every_ticks = Some(2);
    enriched.camera_keys = Some(3);
    enriched.color_mappings = vec!["cell-state".to_owned(), "polarity".to_owned()];
    enriched.field_mappings = vec![
        "trail-carrier".to_owned(),
        "vascular-conductance".to_owned(),
    ];
    enriched.admitted_peak_bytes = Some(750_000_000);
    enriched.frames[0].sim_seconds = Some(0.0);
    enriched.frames[0].mapping = Some("cell-state".to_owned());
    enriched.frames[0].field_mapping = Some("trail-carrier".to_owned());
    enriched.frames[0].camera_center = Some([16.0, 16.0]);
    enriched.frames[0].camera_zoom = Some(2.0);
    let json = serde_json::to_string(&enriched).expect("serializes");
    let back: ExportManifest = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(back, enriched);

    // Ensure TimelineStyle also round-trips all 9 ColorMappings including v2.
    for mapping in ColorMapping::ALL {
        let style = Style {
            mapping,
            look: Look::Sphere,
            ..Style::default()
        };
        let ts = TimelineStyle::from(style);
        assert_eq!(ts.style().expect("parses"), style);
    }
}

/// End-to-end G10 Metal qualification: records a `field-ecology-v2` session
/// cycling through `V2-16` visual mappings (`CellState`, `Polarity`,
/// `TrailCarrier`, `VascularConductance`), exports in `SimulationTime` mode at
/// both **1080p** and **4K** with a torus-spanning `CameraPath`, verifies PNG
/// + y4m + provenance alignment, and confirms tight-budget 4K export refuses
/// without creating `out`.
#[test]
fn g10_cinema_1080p_and_4k_simulation_time_export_with_v2_mappings_and_camera() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let v2_toml = preset_with(
        &[
            ("width", "32.0"),
            ("height", "32.0"),
            ("grid_width", "32"),
            ("grid_height", "32"),
            ("initial_count", "48"),
            ("maximum_count", "256"),
            ("maximum_total_bytes", "2147483648"),
        ],
        "field-ecology-v2",
        true,
    );
    let config = ValidatedConfig::from_toml_with_overrides(
        &v2_toml,
        &[FeatureOverride::Enable("all-v2".to_owned())],
    )
    .expect("valid v2 config");

    let mut studio = Studio::open(
        &config,
        &StudioOptions {
            size: Some([160, 90]),
            cpu_workers: NonZeroUsize::new(2),
            controllers: ControllerChoice::Cpu,
            surface: Surface::Offscreen {
                scale: NonZeroU32::MIN,
            },
            record: true,
            ..StudioOptions::default()
        },
    )
    .expect("opens v2 studio offscreen");

    let mut records = Vec::new();
    let mut statuses = Vec::new();
    let tap = |studio: &mut Studio, code: u16| {
        for pressed in [true, false] {
            studio
                .post_input(InputEvent::Key {
                    code,
                    text: String::new(),
                    pressed,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                })
                .expect("posts key");
        }
    };
    let step_and_drain =
        |studio: &mut Studio, records: &mut Vec<TimelineRecord>, statuses: &mut Vec<Status>| {
            let _ = studio.frame().expect("frame");
            records.extend(studio.take_recorded());
            for status in studio.take_statuses() {
                records.extend(TimelineRecord::status(&status));
                statuses.push(status);
            }
        };
    let wait_step = |studio: &mut Studio,
                     records: &mut Vec<TimelineRecord>,
                     statuses: &mut Vec<Status>,
                     target_steps: usize| {
        let deadline = Instant::now() + Duration::from_secs(30);
        while statuses
            .iter()
            .filter(|s| matches!(s, Status::Stepped { .. }))
            .count()
            < target_steps
        {
            assert!(Instant::now() < deadline, "timed out waiting for step");
            step_and_drain(studio, records, statuses);
            std::thread::sleep(Duration::from_millis(2));
        }
    };

    // Tick 0: CellState + TrailCarrier.
    step_and_drain(&mut studio, &mut records, &mut statuses);
    tap(&mut studio, KEY_8);
    // C three times: Energy -> Composition -> Component -> TrailCarrier.
    for _ in 0..3 {
        tap(&mut studio, KEY_C);
    }
    step_and_drain(&mut studio, &mut records, &mut statuses);

    // Step 2 ticks, apply an energy brush intervention, switch to Polarity + VascularConductance.
    tap(&mut studio, KEY_PERIOD);
    wait_step(&mut studio, &mut records, &mut statuses, 1);
    tap(&mut studio, KEY_E);
    step_and_drain(&mut studio, &mut records, &mut statuses);
    for pressed in [true, false] {
        studio
            .post_input(InputEvent::Button {
                button: PointerButton::Primary,
                pressed,
                position: LogicalPoint { x: 80.0, y: 45.0 },
                clicks: 1,
                modifiers: Modifiers::NONE,
            })
            .expect("posts brush click");
    }
    step_and_drain(&mut studio, &mut records, &mut statuses);
    tap(&mut studio, KEY_9);
    tap(&mut studio, KEY_C); // TrailCarrier -> VascularConductance.
    for target in 2..=4 {
        tap(&mut studio, KEY_PERIOD);
        wait_step(&mut studio, &mut records, &mut statuses, target);
    }
    step_and_drain(&mut studio, &mut records, &mut statuses);

    let closed = studio.close().expect("closes");
    records.push(TimelineRecord::end(&closed.session));
    let text = timeline_text(&records);
    assert_eq!(read_timeline(&text).expect("round-trips"), records);

    let camera = CameraPath {
        keys: vec![
            CameraKey {
                tick: 0,
                center: [30.0, 16.0],
                zoom: 1.0,
            },
            CameraKey {
                tick: 4,
                center: [2.0, 16.0],
                zoom: 2.0,
            },
        ],
    };

    // 1. Cinema 1080p simulation-time export every 2 ticks -> ticks 0, 2, 4 (3 frames).
    let out_1080 = fresh("cinema-1080p");
    let mut opts_1080 = CinemaOptions::cinema_1080p(
        NonZeroU32::new(30).expect("fps"),
        NonZeroU64::new(2).expect("every 2"),
    );
    opts_1080.cpu_workers = NonZeroUsize::new(2);
    opts_1080.camera = Some(camera.clone());
    let manifest_1080 = export_cinema(&config, &text, &opts_1080, &out_1080).expect("1080p export");
    assert_eq!(manifest_1080.mode, "simulation-time");
    assert_eq!(manifest_1080.model.as_deref(), Some("field-ecology-v2"));
    assert!(
        manifest_1080
            .v2_features
            .as_ref()
            .is_some_and(|f| !f.is_empty())
    );
    assert_eq!(manifest_1080.v2_visual_mappings, Some(true));
    assert_eq!(manifest_1080.sample_every_ticks, Some(2));
    assert_eq!(manifest_1080.camera_keys, Some(2));
    assert!(manifest_1080.admitted_peak_bytes.is_some());
    assert_eq!(manifest_1080.digest, closed.session.world.digest().to_hex());
    assert_eq!(manifest_1080.frames.len(), 3);
    assert!(
        manifest_1080
            .color_mappings
            .iter()
            .any(|m| m == "cell-state")
    );
    assert!(manifest_1080.color_mappings.iter().any(|m| m == "polarity"));
    assert!(
        manifest_1080
            .field_mappings
            .iter()
            .any(|m| m == "trail-carrier")
    );
    assert!(
        manifest_1080
            .field_mappings
            .iter()
            .any(|m| m == "vascular-conductance")
    );

    let video_1080 = manifest_1080.video.as_ref().expect("1080p y4m");
    assert_eq!(video_1080.size, CINEMA_1080P);
    assert_eq!(video_1080.slots, 3);
    assert_eq!(video_1080.repeated, 0);
    assert_eq!(video_1080.dropped, 0);
    let y4m_bytes = std::fs::read(out_1080.join(VIDEO_FILE)).expect("y4m");
    assert_eq!(Sha256Digest::of(&y4m_bytes).to_hex(), video_1080.sha256);
    let decoded_y4m = decode_y4m(&y4m_bytes).expect("decodes 1080p y4m");
    assert_eq!(
        (
            decoded_y4m.width,
            decoded_y4m.height,
            decoded_y4m.frames.len()
        ),
        (1920, 1080, 3)
    );
    for (idx, frame) in manifest_1080.frames.iter().enumerate() {
        let expected_tick = idx as u64 * 2;
        assert_eq!(frame.tick, expected_tick);
        assert_eq!(frame.slots, 1);
        let expected_cam = camera.at(expected_tick, [32.0, 32.0]);
        assert_eq!(frame.camera_center, Some(expected_cam.center));
        assert_eq!(frame.camera_zoom, Some(expected_cam.zoom));
        let png = std::fs::read(out_1080.join(&frame.file)).expect("png");
        assert_eq!(Sha256Digest::of(&png).to_hex(), frame.sha256);
        let decoded = decode_png(&png, &PngLimits::default()).expect("decodes png");
        assert_eq!([decoded.width, decoded.height], CINEMA_1080P);
    }
    assert!(out_1080.join(MANIFEST_FILE).exists());

    // 2. Cinema 4K simulation-time export every 4 ticks -> ticks 0, 4 (2 frames).
    let out_4k = fresh("cinema-4k");
    let mut opts_4k = CinemaOptions::cinema_4k(
        NonZeroU32::new(30).expect("fps"),
        NonZeroU64::new(4).expect("every 4"),
    );
    opts_4k.cpu_workers = NonZeroUsize::new(2);
    opts_4k.camera = Some(camera);
    let manifest_4k = export_cinema(&config, &text, &opts_4k, &out_4k).expect("4K export");
    assert_eq!(manifest_4k.frames.len(), 2);
    let video_4k = manifest_4k.video.as_ref().expect("4K y4m");
    assert_eq!(video_4k.size, CINEMA_4K);
    assert_eq!(
        (video_4k.slots, video_4k.repeated, video_4k.dropped),
        (2, 0, 0)
    );
    let png_4k = std::fs::read(out_4k.join(&manifest_4k.frames[0].file)).expect("4K png");
    let decoded_4k = decode_png(&png_4k, &PngLimits::default()).expect("decodes 4K png");
    assert_eq!([decoded_4k.width, decoded_4k.height], CINEMA_4K);
    assert!(
        manifest_4k.admitted_peak_bytes.expect("4K admitted")
            > manifest_1080.admitted_peak_bytes.expect("1080p admitted")
    );

    // 3. Tight-budget 4K export refuses with BudgetExhausted and creates nothing on disk.
    let tight_budget = (manifest_4k.admitted_peak_bytes.expect("admitted") - 1).to_string();
    let tight_toml = preset_with(
        &[
            ("width", "32.0"),
            ("height", "32.0"),
            ("grid_width", "32"),
            ("grid_height", "32"),
            ("initial_count", "48"),
            ("maximum_count", "256"),
            ("maximum_total_bytes", &tight_budget),
        ],
        "field-ecology-v2",
        true,
    );
    let tight_cfg = ValidatedConfig::from_toml_with_overrides(
        &tight_toml,
        &[FeatureOverride::Enable("all-v2".to_owned())],
    )
    .expect("tight v2 config");
    // Re-stamp config_hash so validation passes and FL-043 admission triggers.
    let mut tight_records = records;
    if let Some(TimelineRecord::Header { config_hash, .. }) = tight_records.first_mut() {
        *config_hash = tight_cfg.config_hash().to_hex();
    }
    let tight_text = timeline_text(&tight_records);
    let tight_out = fresh("cinema-4k-refused");
    let err = export_cinema(&tight_cfg, &tight_text, &opts_4k, &tight_out)
        .expect_err("tight-budget 4K export must be refused");
    assert!(
        matches!(err, CoreError::BudgetExhausted { .. }),
        "expected BudgetExhausted, got {err:?}"
    );
    assert!(
        !tight_out.exists(),
        "refused 4K export must not create out/"
    );
}
