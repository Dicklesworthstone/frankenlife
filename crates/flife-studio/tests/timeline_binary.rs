//! FL-023: the binary records a timeline and exports it.
//!
//! - `flife-studio --offscreen 1.5 --timeline T` records its session to T.
//!   T opens with its header and closes with the world the run's `end`
//!   reports, with a `draw` for each frame it rendered.
//! - `flife-studio --export T --out D` opens no Studio. It replays T
//!   headless into D, and its `export` record and manifest close on that
//!   world, with a PNG for each frame and the video on the configuration's
//!   clock.
//! - Neither writes over a file: a second run with the same T or D is
//!   refused, and what is there is unchanged.
//!
//! Every run passes `--offscreen` or `--export`, so none opens a window.
//! Headless Metal: without a Metal device the test returns early. Each run
//! leaves its timeline and export, well under 1 MB, under the target's
//! temporary directory; nothing deletes them.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test: a failure is a test failure; figures are printed"
)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use flife_core::{Sha256Digest, ValidatedConfig};
use flife_studio::{ExportManifest, MANIFEST_FILE, TimelineRecord, VIDEO_FILE, read_timeline};
use fmn_render::metal::MetalRenderer;
use serde_json::Value;

const PRESET: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../presets/reference-small-world.toml"
);

fn studio(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_flife-studio"))
        .arg(PRESET)
        .args(args)
        .output()
        .expect("the binary runs")
}

fn records(output: &std::process::Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .expect("UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON record"))
        .collect()
}

fn the<'a>(records: &'a [Value], kind: &str) -> &'a Value {
    let mut found = records.iter().filter(|record| record["record"] == kind);
    let record = found.next().unwrap_or_else(|| panic!("no `{kind}` record"));
    assert!(found.next().is_none(), "one `{kind}` record");
    record
}

fn path(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

fn sha(path: &Path) -> String {
    Sha256Digest::of(&std::fs::read(path).expect("the file")).to_hex()
}

#[test]
fn a_recorded_session_exports_through_the_binary() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after the epoch")
        .as_nanos();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("fl023-binary-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&dir).expect("a new directory");
    let timeline = dir.join("session.ndjson");
    let record = [
        "--offscreen",
        "1.5",
        "--scale",
        "1",
        "--width",
        "96",
        "--height",
        "60",
        "--controllers",
        "cpu",
        "--cpu-workers",
        "2",
        "--timeline",
        path(&timeline),
    ];
    let output = studio(&record);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "exit {:?}: {stderr}",
        output.status
    );
    let run = records(&output);
    let end = the(&run, "end");
    let digest = end["digest"].as_str().expect("a digest").to_owned();
    let tick = end["tick"].as_u64().expect("a tick");
    let rendered = end["rendered"].as_u64().expect("frames rendered");
    assert!(rendered > 10, "{end}");

    // The timeline: its header, a draw a frame, and the run's world.
    let text = std::fs::read_to_string(&timeline).expect("the timeline");
    let timeline_records = read_timeline(&text).expect("reads");
    let config = ValidatedConfig::from_toml(&std::fs::read_to_string(PRESET).expect("the preset"))
        .expect("valid");
    assert!(matches!(
        &timeline_records[0],
        TimelineRecord::Header { config_hash, .. } if *config_hash == config.config_hash().to_hex()
    ));
    let draws = timeline_records
        .iter()
        .filter(|record| matches!(record, TimelineRecord::Draw { .. }))
        .count() as u64;
    assert_eq!(draws, rendered, "a draw for each frame rendered");
    assert!(
        matches!(
            timeline_records.last(),
            Some(TimelineRecord::End { tick: closed, digest: world, .. })
                if *closed == tick && *world == digest
        ),
        "the timeline closes on the run's world"
    );

    // The export: no Studio, the frames, the video, the same world.
    let out = dir.join("export");
    let output = studio(&[
        "--export",
        path(&timeline),
        "--out",
        path(&out),
        "--cpu-workers",
        "2",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "exit {:?}: {stderr}",
        output.status
    );
    let exported = records(&output);
    assert_eq!(exported.len(), 1, "one record: {exported:?}");
    let summary = the(&exported, "export");
    println!("{summary}");
    assert_eq!(summary["digest"].as_str(), Some(digest.as_str()));
    assert_eq!(summary["end_tick"].as_u64(), Some(tick));
    assert_eq!(summary["frames"].as_u64(), Some(rendered));
    assert_eq!(
        summary["timeline_sha256"].as_str(),
        Some(sha(&timeline).as_str())
    );
    let fps = config.config().presentation.frames_per_second;
    assert_eq!(
        summary["frames_per_second"].as_u64(),
        Some(u64::from(fps)),
        "the configuration's rate by default"
    );
    let manifest: ExportManifest =
        serde_json::from_slice(&std::fs::read(out.join(MANIFEST_FILE)).expect("the manifest"))
            .expect("parses");
    assert_eq!(manifest.digest, digest);
    assert_eq!(manifest.frames.len() as u64, rendered);
    for frame in &manifest.frames {
        assert_eq!(sha(&out.join(&frame.file)), frame.sha256, "{}", frame.file);
    }
    let video = manifest.video.as_ref().expect("a video at 96 x 60");
    assert_eq!(summary["slots"].as_u64(), Some(video.slots));
    assert_eq!(sha(&out.join(VIDEO_FILE)), video.sha256);

    // Neither is written over.
    let (timeline_sha, manifest_sha) = (sha(&timeline), sha(&out.join(MANIFEST_FILE)));
    let output = studio(&record);
    assert_eq!(output.status.code(), Some(2), "the timeline exists");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("never written over"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = studio(&["--export", path(&timeline), "--out", path(&out)]);
    assert_eq!(output.status.code(), Some(1), "the export exists");
    assert_eq!(
        (sha(&timeline), sha(&out.join(MANIFEST_FILE))),
        (timeline_sha, manifest_sha)
    );
}
