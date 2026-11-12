//! FL-024: `flife-studio --offscreen SECONDS`, the binary's own loop with
//! no window. Its records are a window's, from `surface: offscreen`; its
//! frames are rendered and read back; its `end` carries the frames' times;
//! and it closes on a world that is the directly stepped one, bit for bit.
//! Every run here passes `--offscreen`, so none opens a window; the
//! arguments it refuses are checked on the parser (`src/main.rs`). Headless Metal: without a Metal device the tests
//! return early.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test: a failure is a test failure; figures are printed"
)]

use std::process::Command;

use flife_core::ValidatedConfig;
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
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

fn number(record: &Value, key: &str) -> u64 {
    record[key]
        .as_u64()
        .unwrap_or_else(|| panic!("`{key}` in {record}"))
}

fn float(record: &Value, key: &str) -> f64 {
    record[key]
        .as_f64()
        .unwrap_or_else(|| panic!("`{key}` in {record}"))
}

#[test]
fn an_offscreen_run_reports_its_frames_and_closes_on_the_direct_world() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let output = studio(&[
        "--offscreen",
        "2.5",
        "--offscreen-zooms",
        "--scale",
        "2",
        "--width",
        "320",
        "--height",
        "200",
        "--controllers",
        "cpu",
        "--cpu-workers",
        "2",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "exit {:?}: {stderr}",
        output.status
    );
    let records: Vec<Value> = String::from_utf8(output.stdout)
        .expect("UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON record"))
        .collect();
    let kind = |record: &Value| record["record"].as_str().unwrap_or("").to_owned();
    let open = &records[0];
    assert_eq!(kind(open), "open");
    assert_eq!(open["surface"], "offscreen");
    // Plan §13.4: the affinity projection's matrix and scaling, recorded.
    let fixed = flife_render::AffinityProjection::fixed();
    assert_eq!(
        open["projection"],
        serde_json::json!({
            "rows": fixed.rows,
            "matrix": fixed.matrix,
            "divisor": fixed.divisor,
            "affinity_limit": 1.0
        })
    );
    // As a window's: the view at the content's size in points, before the
    // first frame refits it to the drawable.
    assert_eq!(open["window"], serde_json::json!([320, 200]));
    // Admitted as a window's Studio is (FL-043): its observers charged for
    // the drawable in pixels, and on the CPU reference no backend budget.
    let memory = &open["memory"];
    let terms = [
        "genomes",
        "state",
        "field",
        "scratch",
        "births",
        "render",
        "checkpoint",
        "reserve",
        "backend",
    ];
    let peak = number(memory, "peak_bytes");
    assert_eq!(
        terms.iter().map(|term| number(memory, term)).sum::<u64>(),
        peak,
        "{memory}"
    );
    assert!(peak <= number(memory, "budget_bytes"), "{memory}");
    assert_eq!(number(memory, "backend"), 0, "{memory}");
    // The same content at one pixel a point charges its observers at least
    // the pixels' device bytes fewer (640 x 400 against 320 x 200).
    let single = studio(&[
        "--offscreen",
        "0.1",
        "--scale",
        "1",
        "--width",
        "320",
        "--height",
        "200",
        "--controllers",
        "cpu",
        "--cpu-workers",
        "2",
    ]);
    assert!(single.status.success(), "{single:?}");
    let single: Value = serde_json::from_str(
        String::from_utf8(single.stdout)
            .expect("UTF-8")
            .lines()
            .next()
            .expect("an open record"),
    )
    .expect("a JSON record");
    let fewer = number(memory, "render") - number(&single["memory"], "render");
    assert!(
        fewer >= (640 * 400 - 320 * 200) * flife_studio::memory::DEVICE_BYTES_PER_PIXEL,
        "{fewer} B fewer: {memory} against {}",
        single["memory"]
    );
    assert!(
        records
            .iter()
            .any(|record| kind(record) == "status" && record["status"] == "running"),
        "the world runs on open"
    );
    let summaries: Vec<&Value> = records
        .iter()
        .filter(|record| kind(record) == "frame")
        .collect();
    assert!(!summaries.is_empty(), "a frame summary a second");
    for summary in &summaries {
        assert_eq!(number(summary, "presented"), 0);
        assert_eq!(number(summary, "rendered"), number(summary, "frames"));
        assert!(number(summary, "readback_bytes") >= number(summary, "rendered") * 640 * 400 * 4);
    }
    // The preset reports its clocks: the frames are the ones drawn.
    let clocks: Vec<&Value> = records
        .iter()
        .filter(|record| kind(record) == "clock")
        .collect();
    assert!(!clocks.is_empty(), "a clock record a second");
    assert!(clocks.iter().all(|clock| number(clock, "frames") > 0));

    let end = records.last().expect("records");
    assert_eq!(kind(end), "end");
    let frames = number(end, "frames");
    assert!(frames >= 10, "{frames} frames in 2.5 s");
    assert_eq!(number(end, "rendered"), frames);
    assert_eq!(number(end, "presented"), 0);
    assert_eq!(number(end, "input_dropped"), 0);
    assert_eq!(number(end, "keys"), 0, "only camera moves are posted");
    // A move in every third frame's interval, each drawn by the next frame
    // (the last may arrive after it).
    let moves = number(end, "camera_moves");
    assert!(
        moves >= frames / 3 - 1 && moves <= frames / 3 + 1,
        "{moves} moves, {frames} frames"
    );
    assert_eq!(
        number(end, "camera_moves_unmoved"),
        0,
        "every move moved the view"
    );
    let latency = &end["camera_latency"];
    let drawn = number(latency, "frames");
    assert!(
        drawn == moves || drawn + 1 == moves,
        "{drawn} of {moves} moves drawn"
    );
    let [l50, l95, lmax] = ["p50_ms", "p95_ms", "max_ms"].map(|key| float(latency, key));
    assert!(0.0 < l50 && l50 <= l95 && l95 <= lmax, "{latency}");
    let wall = float(end, "wall_seconds");
    assert!(wall >= 2.5, "{wall} s");
    let times = &end["frame_times"];
    assert_eq!(number(times, "frames"), frames);
    let [p50, p95, p99, max] =
        ["p50_ms", "p95_ms", "p99_ms", "max_ms"].map(|key| float(times, key));
    assert!(
        0.0 < p50 && p50 <= p95 && p95 <= p99 && p99 <= max,
        "{times}"
    );
    println!(
        "offscreen 640x400 at 2x for {wall:.2} s: {frames} frames, tick {}; frame work p50 {p50:.1} \
         ms, p95 {p95:.1}, p99 {p99:.1}, max {max:.1}; {moves} camera moves, arrival to drawn \
         p50 {l50:.1} ms, p95 {l95:.1}, max {lmax:.1}",
        number(end, "tick")
    );

    // The world it closed on is the directly stepped one.
    let config = ValidatedConfig::from_toml(&std::fs::read_to_string(PRESET).expect("the preset"))
        .expect("valid");
    let mut direct = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("the profile"),
    )
    .expect("founds");
    for _ in 0..number(end, "tick") {
        match direct.step().expect("steps") {
            Stepped::Published(_) => {}
            Stepped::Paused(shortfall) => panic!("the direct world paused: {shortfall:?}"),
        }
    }
    assert_eq!(end["digest"], direct.digest().to_hex());
}
