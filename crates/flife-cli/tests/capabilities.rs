//! `flife capabilities` (plan §14.6, §20.5): what this machine and build can
//! run, each controller route run against the reference, with explicit
//! refusals. Also `flife render --video`'s encoder, checked before the first
//! frame.
//!
//! What these tests do not show: the Studio's window (never opened here),
//! and a route that disagrees with the reference on a real host. The
//! classification of a disagreement is unit-tested in `capabilities.rs`.
//! On a host without Metal, the Metal route is expected `unavailable`; on
//! one with it, `verified`. Each test asks the device itself which applies.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use flife_backend_metal::MetalControllers;
use flife_cli::{
    Admitted, CAPABILITIES_SCHEMA, CapabilitiesOptions, ControllerChoice, ERROR_SCHEMA,
    PRESET_DIGEST, PROBE_TICKS, Route, RouteStatus, RunOptions, Stop, capabilities, found, run,
    video_encoder,
};
use flife_core::{CoreError, ValidatedConfig};
use serde_json::Value;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");
const PRESET_BUDGET: &str = "maximum_total_bytes = 2147483648";

fn preset_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../presets/reference-small-world.toml")
}

fn scratch(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("flife-capabilities")
        .join(name);
    if path.exists() {
        std::fs::remove_dir_all(&path).expect("clears the last run's directory");
    }
    std::fs::create_dir_all(&path).expect("creates the scratch directory");
    path
}

/// Whether this host opens a Metal device, asked of the device itself.
fn metal_device() -> bool {
    match MetalControllers::open() {
        Ok(_) => true,
        Err(CoreError::MissingCapability { .. }) => false,
        Err(error) => panic!("a Metal device that does not compile its kernels: {error}"),
    }
}

fn route<'a>(routes: &'a [Route], name: &str) -> &'a Route {
    routes
        .iter()
        .find(|route| route.route == name)
        .unwrap_or_else(|| panic!("a `{name}` route in {routes:?}"))
}

/// The preset with `[execution] maximum_total_bytes` set to `budget`.
fn preset_with_budget(budget: u64) -> ValidatedConfig {
    assert!(PRESET.contains(PRESET_BUDGET), "the preset's budget line");
    let text = PRESET.replace(PRESET_BUDGET, &format!("maximum_total_bytes = {budget}"));
    ValidatedConfig::from_toml(&text).expect("the preset validates with another budget")
}

/// The preset's terms other than the backend's, without and with a
/// controller backend.
fn fixed_terms() -> (u64, u64) {
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let world = found(&config).expect("founds");
    let reference = world.admit_memory(false, 0, 0).expect("admitted");
    let batched = world.admit_memory(true, 0, 0).expect("admitted");
    (reference.fixed_bytes(), batched.fixed_bytes())
}

fn flife(args: &[&str], path: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_flife"));
    command.args(args);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    command.output().expect("flife runs")
}

fn record(bytes: &[u8]) -> Value {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "one record: {text}");
    serde_json::from_str(lines[0]).expect("JSON")
}

#[test]
fn the_preset_reaches_its_pinned_world_on_every_route_this_host_opens() {
    // The pinned world is the one a session run reaches, not only the
    // capability report's own stepping.
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let finished = run(
        &config,
        RunOptions {
            ticks: PROBE_TICKS,
            every: 0,
            cpu_workers: None,
            controllers: ControllerChoice::Cpu,
            metrics_every: 0,
        },
        |_| {},
    )
    .expect("runs");
    assert_eq!(finished.stop, Stop::Reached);
    assert_eq!(finished.closed.world.digest().to_hex(), PRESET_DIGEST);

    let report = capabilities(None, &CapabilitiesOptions::default()).expect("reports");
    assert_eq!(report.schema, CAPABILITIES_SCHEMA);
    assert_eq!(report.build.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(report.build.os, std::env::consts::OS);
    assert!(report.host.logical_cores >= 1);
    assert_eq!(report.world.source, "preset");
    assert_eq!(report.world.founders, 4096);
    assert_eq!(report.world.ticks, PROBE_TICKS);
    assert_eq!(report.world.pinned_digest, Some(PRESET_DIGEST));
    assert_eq!(report.world.config_hash, config.config_hash().to_hex());
    let Admitted::Admitted(memory) = &report.world.memory else {
        panic!("the preset is admitted: {:?}", report.world.memory);
    };
    assert!(memory.backend > 0, "auto's admission grants a backend");
    assert_eq!(memory.peak_bytes, memory.budget_bytes);

    let names: Vec<&str> = report.controllers.iter().map(|route| route.route).collect();
    assert_eq!(names, ["cpu", "torch", "metal"]);
    let cpu = route(&report.controllers, "cpu");
    assert_eq!(cpu.status, RouteStatus::Verified, "{cpu:?}");
    assert_eq!(cpu.tick, Some(PROBE_TICKS));
    assert_eq!(cpu.digest.as_deref(), Some(PRESET_DIGEST));
    let torch = route(&report.controllers, "torch");
    assert_eq!(torch.status, RouteStatus::Verified, "{torch:?}");
    assert_eq!(torch.digest.as_deref(), Some(PRESET_DIGEST));
    let summary = torch.controllers.as_ref().expect("a run");
    assert_eq!(
        (summary.batched, summary.deferred),
        (4096 * PROBE_TICKS, 0),
        "every founder, every tick, on the route"
    );

    let device = metal_device();
    let metal = route(&report.controllers, "metal");
    if device {
        assert_eq!(metal.status, RouteStatus::Verified, "{metal:?}");
        assert_eq!(metal.digest.as_deref(), Some(PRESET_DIGEST));
        let summary = metal.controllers.as_ref().expect("a run");
        assert!(summary.backend.starts_with("metal ("), "{summary:?}");
        assert!(summary.batched > 0);
    } else {
        assert_eq!(metal.status, RouteStatus::Unavailable, "{metal:?}");
        assert_eq!(metal.kind, Some("missing-capability"));
        assert!(metal.digest.is_none());
    }
    assert_eq!(report.auto.metal_device, Some(device));
    let auto = ControllerChoice::auto_for(report.auto.cpu_workers, device);
    let expected = match auto {
        ControllerChoice::Metal => "metal",
        ControllerChoice::Torch => "torch",
        ControllerChoice::Cpu | ControllerChoice::Auto => "cpu",
    };
    assert_eq!(report.auto.takes, Some(expected));
    assert_eq!(report.auto.cpu_workers, 1, "the preset's workers");
    assert!(!report.defect());
    assert!(!report.refused());

    match video_encoder() {
        Ok(version) => {
            assert_eq!(report.video.status, "available");
            assert_eq!(report.video.version.as_deref(), Some(version.as_str()));
        }
        Err(error) => {
            assert_eq!(report.video.status, "unavailable");
            assert_eq!(report.video.detail, Some(error.to_string()));
        }
    }
}

#[test]
fn a_route_left_no_budget_defers_everything_and_is_idle_not_verified() {
    // The batched path's terms exactly: the backend is granted nothing, so
    // it makes no shard and defers every evaluation. Its world is the
    // reference's, but it did no work.
    let (_, batched) = fixed_terms();
    let config = preset_with_budget(batched);
    let report = capabilities(Some(&config), &CapabilitiesOptions::default()).expect("reports");
    assert_eq!(report.world.source, "config");
    assert_eq!(
        report.world.pinned_digest, None,
        "only the preset is pinned"
    );
    let Admitted::Admitted(memory) = &report.world.memory else {
        panic!("admitted: {:?}", report.world.memory);
    };
    assert_eq!(memory.backend, 0);

    let cpu = route(&report.controllers, "cpu");
    assert_eq!(cpu.status, RouteStatus::Reference, "{cpu:?}");
    // The budget changes nothing the world computes.
    assert_eq!(cpu.digest.as_deref(), Some(PRESET_DIGEST));
    let torch = route(&report.controllers, "torch");
    assert_eq!(torch.status, RouteStatus::Idle, "{torch:?}");
    assert_eq!(torch.digest.as_deref(), Some(PRESET_DIGEST));
    let summary = torch.controllers.as_ref().expect("a run");
    assert_eq!((summary.batched, summary.deferred), (0, 4096 * PROBE_TICKS));
    assert!(
        torch
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("did no work")),
        "{torch:?}"
    );
    let metal = route(&report.controllers, "metal");
    let expected = if metal_device() {
        RouteStatus::Idle
    } else {
        RouteStatus::Unavailable
    };
    assert_eq!(metal.status, expected, "{metal:?}");
    assert!(!report.defect());
    assert!(!report.refused());
}

#[test]
fn a_budget_with_no_room_for_batching_refuses_the_backends_and_auto_but_not_the_reference() {
    let (reference, batched) = fixed_terms();
    assert!(batched > reference, "batching holds lists of its own");
    let config = preset_with_budget(reference);
    let report = capabilities(Some(&config), &CapabilitiesOptions::default()).expect("reports");
    let cpu = route(&report.controllers, "cpu");
    assert_eq!(cpu.status, RouteStatus::Reference, "{cpu:?}");
    assert_eq!(cpu.digest.as_deref(), Some(PRESET_DIGEST));
    for name in ["torch", "metal"] {
        let backend = route(&report.controllers, name);
        assert_eq!(backend.status, RouteStatus::Refused, "{backend:?}");
        assert_eq!(backend.kind, Some("budget-exhausted"));
        assert!(backend.digest.is_none());
    }
    // `flife run`'s default (`auto`) is batched, so it is refused too.
    let Admitted::Refused(refusal) = &report.world.memory else {
        panic!("auto is refused: {:?}", report.world.memory);
    };
    assert_eq!(refusal.kind, "budget-exhausted");
    assert_eq!((report.auto.metal_device, report.auto.takes), (None, None));
    assert!(!report.defect());
    assert!(!report.refused(), "the reference runs");
}

#[test]
fn the_binary_reports_exits_by_what_it_found_and_refuses_explicitly() {
    let output = flife(&["capabilities"], None);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = record(&output.stdout);
    assert_eq!(report["schema"], CAPABILITIES_SCHEMA);
    assert_eq!(report["controllers"][0]["status"], "verified");
    assert_eq!(report["world"]["pinned_digest"], PRESET_DIGEST);

    // A configuration whose world the reference cannot admit: the report
    // is printed with every route refused, and the exit status is 2.
    let (reference, _) = fixed_terms();
    let dir = scratch("refused");
    let config = dir.join("too-small.toml");
    let text = PRESET.replace(
        PRESET_BUDGET,
        &format!("maximum_total_bytes = {}", reference - 1),
    );
    std::fs::write(&config, text).expect("writes");
    let output = flife(
        &["capabilities", "--config", config.to_str().expect("UTF-8")],
        None,
    );
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let report = record(&output.stdout);
    for route in report["controllers"].as_array().expect("routes") {
        assert_eq!(route["status"], "refused", "{route}");
        assert_eq!(route["kind"], "budget-exhausted", "{route}");
    }
    assert!(report["world"]["memory"]["refused"].is_object());

    // An unreadable configuration: an error record, no report.
    let missing = dir.join("absent.toml");
    let output = flife(
        &["capabilities", "--config", missing.to_str().expect("UTF-8")],
        None,
    );
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty());
    let error = record(&output.stderr);
    assert_eq!(
        (error["schema"].as_str(), error["kind"].as_str()),
        (Some(ERROR_SCHEMA), Some("io"))
    );

    // No probe of zero ticks.
    let output = flife(&["capabilities", "--ticks", "0"], None);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty());
}

/// A stand-in `ffmpeg` in its own directory, answering `-encoders` with
/// `encoders` and exiting with `status`.
#[cfg(unix)]
fn fake_ffmpeg(name: &str, encoders: &str, status: i32) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch(name);
    let script = dir.join("ffmpeg");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncase \"$*\" in\n  *-encoders*) printf '%s\\n' '{encoders}' ;;\n  \
             *) echo 'ffmpeg version stand-in' ;;\nesac\nexit {status}\n"
        ),
    )
    .expect("writes");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    dir
}

#[cfg(unix)]
#[test]
fn video_encoding_is_reported_as_the_path_offers_it() {
    let cases = [
        (
            fake_ffmpeg(
                "with-libx264",
                " V....D libx264              libx264 H.264",
                0,
            ),
            "available",
            None,
        ),
        (
            fake_ffmpeg(
                "without-libx264",
                " V....D libx265              libx265 H.265",
                0,
            ),
            "unavailable",
            Some("lists none"),
        ),
        (
            fake_ffmpeg("failing", "", 1),
            "unavailable",
            Some("exited with"),
        ),
        (
            scratch("no-ffmpeg"),
            "unavailable",
            Some("ffmpeg, to encode a video"),
        ),
    ];
    for (path, status, detail) in &cases {
        let output = flife(&["capabilities"], Some(path));
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let report = record(&output.stdout);
        let video = &report["video"];
        assert_eq!(video["status"], *status, "{}: {video}", path.display());
        match detail {
            None => assert_eq!(video["version"], "ffmpeg version stand-in"),
            Some(detail) => assert!(
                video["detail"]
                    .as_str()
                    .is_some_and(|text| text.contains(detail)),
                "{}: {video}",
                path.display()
            ),
        }
        // Without `sysctl` or `/proc` on the path, the host's memory may be
        // unknown; the report says so by leaving the comparison out.
        if report["host"]["memory_bytes"].is_null() {
            assert!(report["world"]["budget_within_physical_memory"].is_null());
        }
    }
}

#[test]
fn a_video_render_is_refused_before_its_first_frame_without_an_encoder() {
    let dir = scratch("render-refused");
    let frames = dir.join("frames");
    let preset = preset_path();
    let video = dir.join("out.mp4");
    let args = [
        "render",
        preset.to_str().expect("UTF-8"),
        "--ticks",
        "2",
        "--every",
        "1",
        "--width",
        "32",
        "--height",
        "32",
        "--out",
        frames.to_str().expect("UTF-8"),
        "--video",
        video.to_str().expect("UTF-8"),
    ];
    let empty = scratch("render-empty-path");
    let output = flife(&args, Some(&empty));
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "no start record: nothing founded");
    let error = record(&output.stderr);
    assert_eq!(error["kind"], "missing-capability", "{error}");
    assert!(!frames.exists(), "no frame directory");
    assert!(!video.exists());

    // Where this host has the encoder, the same render writes the video.
    if video_encoder().is_ok() {
        let output = flife(&args, None);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let mp4 = std::fs::read(&video).expect("the video exists");
        assert_eq!(&mp4[4..8], b"ftyp", "an MP4 begins with its ftyp box");
    }
}
