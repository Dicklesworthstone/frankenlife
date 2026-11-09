//! FL-001 acceptance: each capability probe qualifies against its oracle on
//! every host, except Metal, which is qualified on macOS and reported
//! unavailable elsewhere. Native input is typed on every host since FL-018;
//! the window half runs from the `flife-probe --window` binary.

use flife_probes::{ProbeReport, Verdict};

fn assert_qualified(report: &ProbeReport) {
    assert_eq!(
        report.verdict,
        Verdict::Qualified,
        "{} probe: {} (evidence {:?})",
        report.probe,
        report.verdict,
        report.evidence
    );
}

#[test]
fn closure_resolves_one_pinned_universe() {
    assert_qualified(&flife_probes::closure_probe(
        flife_probes::CARGO_LOCK,
        flife_probes::STACK_LOCK,
    ));
}

#[test]
fn asupersync_runtime_and_lab_replay() {
    assert_qualified(&flife_probes::runtime_probe());
}

#[test]
fn rng_matches_numpy() {
    assert_qualified(&flife_probes::rng_probe());
}

#[test]
fn field_layout_matches_numpy() {
    assert_qualified(&flife_probes::array_probe());
}

#[test]
fn tensor_matmul_is_exact() {
    assert_qualified(&flife_probes::tensor_probe());
}

#[test]
fn manim_renders_a_reproducible_dot_cloud() {
    let report =
        flife_probes::manim_frame_probe(std::path::Path::new(env!("CARGO_TARGET_TMPDIR")), "test");
    assert_qualified(&report);
}

#[test]
fn metal_is_qualified_on_macos_and_unavailable_elsewhere() {
    let report = flife_probes::metal_probe();
    if cfg!(target_os = "macos") {
        assert_qualified(&report);
    } else {
        assert!(
            matches!(report.verdict, Verdict::Unavailable(_)),
            "off macOS the Metal probe must be unavailable, not {}",
            report.verdict
        );
    }
}

#[test]
fn native_input_is_typed_and_bounded() {
    let report = flife_probes::native_input_probe();
    assert_qualified(&report);
    let kinds = report
        .evidence
        .iter()
        .find(|(key, _)| *key == "input_events")
        .map(|(_, value)| value.as_str());
    assert_eq!(
        kinds,
        Some(
            "pointer-moved,button,scroll,key,modifiers-changed,focus-gained,focus-lost,\
             pointer-capture-lost,resized"
        )
    );
}

#[test]
fn a_drifted_pin_is_caught() {
    let drifted = flife_probes::STACK_LOCK.replace(
        "6492ff310c45ad72abf27e4394f68b9059770193",
        "0000000000000000000000000000000000000000",
    );
    let report = flife_probes::closure_probe(flife_probes::CARGO_LOCK, &drifted);
    assert!(
        report.failed(),
        "a drifted franken_numpy pin must fail the closure probe"
    );
}

#[test]
fn a_second_asupersync_copy_is_caught() {
    let doubled = format!(
        "{}\n[[package]]\nname = \"asupersync\"\nversion = \"0.6.2\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
        flife_probes::CARGO_LOCK
    );
    let report = flife_probes::closure_probe(&doubled, flife_probes::STACK_LOCK);
    assert!(
        report.failed(),
        "an unpatched crates.io asupersync must fail the closure probe"
    );
}

#[test]
fn ndjson_lines_are_single_line_objects() {
    let line = flife_probes::native_input_probe().to_json_line();
    assert!(line.starts_with("{\"schema\":\"flife.probe.v1\""));
    assert!(line.ends_with('}'));
    assert!(!line.contains('\n'));
}
