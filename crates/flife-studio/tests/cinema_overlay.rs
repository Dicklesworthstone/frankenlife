//! A simulation-time Cinema export (plan §13.8, FL-050) draws the world at
//! exact sampled ticks. The overlay a live session recorded is that session's
//! readout at its own ticks and speeds, so drawn over a sampled frame it
//! misstates it: FL-024's G5 review found a frame of tick 1,280 reading
//! `running · tick 1,271 · 34.4 ticks/s · 0.27× real time · 25 fps`. A
//! sampled frame carries no recorded overlay; a live capture still draws it.

#![allow(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "integration tests: an unexpected failure is a test failure"
)]

use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use flife_core::ValidatedConfig;
use flife_engine::{Arrangement, NumericalProfile, World};
use flife_studio::{
    CinemaOptions, Exact, ExportMode, Style, TimelineRecord, export_cinema, write_timeline_record,
};
use fmn_render::metal::MetalRenderer;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

fn fresh(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("after epoch")
        .as_nanos();
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "cinema-overlay-{name}-{}-{nanos}",
        std::process::id()
    ))
}

/// The SHA-256 of the one frame `records` export to under `options`.
fn frame(
    config: &ValidatedConfig,
    records: &[TimelineRecord],
    options: &CinemaOptions,
    name: &str,
) -> String {
    let mut bytes = Vec::new();
    for record in records {
        write_timeline_record(&mut bytes, record).expect("writes the record");
    }
    let text = String::from_utf8(bytes).expect("utf-8 timeline");
    let manifest = export_cinema(config, &text, options, &fresh(name)).expect("exports");
    assert_eq!(manifest.frames.len(), 1, "{name}");
    manifest.frames[0].sha256.clone()
}

#[test]
fn simulation_time_frames_carry_no_recorded_overlay() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let config = ValidatedConfig::from_toml(PRESET).expect("the small preset validates");
    let world = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    let header = TimelineRecord::header(&config, [160, 100], Style::default());
    let overlay = TimelineRecord::Overlay {
        text: "running · tick 1,271\n34.4 ticks/s · 0.27× real time · 25 fps".to_owned(),
        swatches: vec![[255, 64, 32]],
    };
    let draw = TimelineRecord::Draw {
        frame: 0,
        tick: 0,
        wall_seconds: Exact(0.0),
    };
    let end = TimelineRecord::End {
        tick: 0,
        digest: world.digest().to_hex(),
        applied: Vec::new(),
    };
    let plain = [header.clone(), draw.clone(), end.clone()];
    let with_overlay = [header, overlay, draw, end];

    let live = CinemaOptions {
        frames_per_second: NonZeroU32::new(30).expect("30 fps"),
        cpu_workers: NonZeroUsize::new(2),
        mp4: false,
        mode: ExportMode::LiveCapture,
        resolution: Some([320, 200]),
        pixels_per_point: Some(1.0),
        camera: None,
    };
    let sampled = CinemaOptions {
        mode: ExportMode::SimulationTime {
            every_ticks: NonZeroU64::new(1).expect("1"),
        },
        ..live.clone()
    };

    assert_ne!(
        frame(&config, &with_overlay, &live, "live-overlay"),
        frame(&config, &plain, &live, "live-plain"),
        "a live capture draws the overlay it recorded"
    );
    assert_eq!(
        frame(&config, &with_overlay, &sampled, "sampled-overlay"),
        frame(&config, &plain, &sampled, "sampled-plain"),
        "a simulation-time frame carries no recorded overlay"
    );
}
