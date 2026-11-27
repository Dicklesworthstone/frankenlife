//! FL-049 evidence (plan §2.4, §15.5, §18.5, Gates G5 and G9): validates the
//! provenance-backed visual gallery manifest (`gallery/manifest.json`) and
//! proves `INV-09` presentation purity across live aesthetic + paired raw G5
//! renders.
//!
//! 1. Verifies `gallery/manifest.json` covers all six required plan §2.4
//!    categories (`dense-world`, `deep-zoom`, `interacting-boundary`,
//!    `perturbed-colony`, `lineage-view`, `raw-field-view`), declares complete
//!    provenance (run ID, seed, preset + SHA-256, model version, tick, verified
//!    digest, selection procedure, and paired raw view), and references only
//!    valid committed presets and camera files.
//! 2. Executes a live paired aesthetic + raw G5 `flife_cli::render` run on a
//!    compact reference world, verifying both PNG frames are produced and that
//!    changing look/color/camera/field mappings leaves the committed
//!    `World::digest()` bit-for-bit identical (`INV-09`).

#![forbid(unsafe_code)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests: an unexpected failure is a test failure"
)]

use std::collections::BTreeSet;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use flife_cli::{
    CameraPath, ColorMapping, ControllerChoice, FieldMapping, Look, Particles, RenderOptions,
    RenderRecord, Style, render, validate_v2_visual_style,
};
use flife_core::ValidatedConfig;
use flife_core::canonical::Sha256Digest;
use serde_json::Value;

const MANIFEST: &str = include_str!("../../../gallery/manifest.json");
const REFERENCE: &str = include_str!("../../../presets/reference-small-world.toml");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn scratch_dir(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("flife-fl049-gallery")
        .join(format!("{label}-{}-{n}", std::process::id()));
    fs::create_dir_all(&dir).expect("create gallery scratch dir");
    dir
}

/// Validates `gallery/manifest.json` against every plan §2.4 / §18.5 / G5
/// provenance requirement: all six categories, real committed presets with
/// exact SHA-256 matches, valid camera paths, and paired raw G5 views.
#[test]
fn gallery_manifest_covers_all_six_categories_with_valid_provenance() {
    let root = repo_root();
    let doc: Value = serde_json::from_str(MANIFEST).expect("valid gallery/manifest.json");
    assert_eq!(
        doc.get("schema").and_then(Value::as_str),
        Some("frankenlife.gallery.v1")
    );
    assert!(
        doc.get("selection_procedure")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty()),
        "gallery must state its selection procedure (plan §15.5)"
    );

    let entries = doc
        .get("entries")
        .and_then(Value::as_array)
        .expect("entries array");
    assert!(
        entries.len() >= 6,
        "plan §2.4 requires at least 6 gallery recordings, found {}",
        entries.len()
    );

    let mut categories = BTreeSet::new();
    let mut run_ids = BTreeSet::new();

    for entry in entries {
        let id = entry.get("id").and_then(Value::as_str).expect("entry id");
        let cat = entry
            .get("category")
            .and_then(Value::as_str)
            .expect("entry category");
        let run_id = entry
            .get("run_id")
            .and_then(Value::as_str)
            .expect("entry run_id");
        let preset_rel = entry
            .get("preset")
            .and_then(Value::as_str)
            .expect("entry preset");
        let preset_sha = entry
            .get("preset_sha256")
            .and_then(Value::as_str)
            .expect("entry preset_sha256");
        let model = entry
            .get("model")
            .and_then(Value::as_str)
            .expect("entry model");
        let seed = entry
            .get("seed")
            .and_then(Value::as_u64)
            .expect("entry seed");
        let digest = entry
            .get("verified_world_digest")
            .and_then(Value::as_str)
            .expect("entry verified_world_digest");

        assert!(
            run_ids.insert(run_id.to_owned()),
            "duplicate run_id {run_id}"
        );
        categories.insert(cat.to_owned());
        assert_eq!(digest.len(), 64, "digest for {id} must be 64-hex SHA-256");
        assert!(
            digest.bytes().all(|b| b.is_ascii_hexdigit()),
            "digest for {id} must be hex"
        );

        let preset_path = root.join(preset_rel);
        let preset_bytes = fs::read(&preset_path)
            .unwrap_or_else(|e| panic!("gallery entry {id} missing preset {preset_rel}: {e}"));
        let actual_sha = Sha256Digest::of(&preset_bytes).to_hex();
        assert_eq!(
            actual_sha, preset_sha,
            "preset SHA-256 drift for gallery entry {id} ({preset_rel})"
        );
        let preset_text = String::from_utf8(preset_bytes).expect("UTF-8 preset");
        let validated = ValidatedConfig::from_toml(&preset_text)
            .unwrap_or_else(|e| panic!("gallery preset {preset_rel} failed validation: {e}"));
        assert_eq!(validated.config().model, model);
        assert_eq!(validated.config().seed, seed);
        let tick = entry
            .get("checkpoint_or_tick")
            .and_then(Value::as_u64)
            .expect("entry checkpoint_or_tick");
        assert!(tick > 0, "gallery entry {id} must specify tick > 0");
        let eco_rel = entry
            .get("ecological_reference")
            .and_then(Value::as_str)
            .expect("entry ecological_reference");
        assert!(
            root.join(eco_rel).is_file(),
            "gallery entry {id} missing ecological_reference {eco_rel}"
        );
        if cat == "perturbed-colony" {
            assert!(
                entry
                    .get("interventions")
                    .and_then(Value::as_array)
                    .is_some_and(|arr| !arr.is_empty()),
                "perturbed-colony gallery entry {id} must list its declared interventions"
            );
        }

        // Both aesthetic and raw G5 views must be present and parse through the
        // real CLI/Manim mapping parsers + V2 visual compatibility validator.
        let aesthetic = entry.get("aesthetic_view").expect("aesthetic_view");
        let raw = entry.get("raw_view").expect("raw_view");
        assert_eq!(
            raw.get("look").and_then(Value::as_str),
            Some("raw"),
            "gallery entry {id} must pair an honest raw G5 view"
        );
        for view in [aesthetic, raw] {
            let look_str = view.get("look").and_then(Value::as_str).expect("view look");
            Look::from_str(look_str)
                .unwrap_or_else(|e| panic!("gallery entry {id} invalid look {look_str:?}: {e}"));
            let color_str = view
                .get("color_mapping")
                .and_then(Value::as_str)
                .expect("view color_mapping");
            let color = ColorMapping::from_str(color_str).unwrap_or_else(|e| {
                panic!("gallery entry {id} invalid color_mapping {color_str:?}: {e}")
            });
            let field_str = view
                .get("field_mapping")
                .and_then(Value::as_str)
                .expect("view field_mapping");
            let field = FieldMapping::from_str(field_str).unwrap_or_else(|e| {
                panic!("gallery entry {id} invalid field_mapping {field_str:?}: {e}")
            });
            validate_v2_visual_style(&validated, color, field).unwrap_or_else(|e| {
                panic!(
                    "gallery entry {id} incompatible style ({color_str}, {field_str}) for {preset_rel}: {e}"
                )
            });
            assert!(
                view.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.contains("flife render")),
                "gallery entry {id} missing reproducible flife render command"
            );
            if let Some(cam_rel) = view.get("camera").and_then(Value::as_str) {
                let cam_path = root.join(cam_rel);
                let cam_text = fs::read_to_string(&cam_path)
                    .unwrap_or_else(|e| panic!("missing camera file {cam_rel}: {e}"));
                let cam: Value = serde_json::from_str(&cam_text).expect("valid camera JSON");
                assert!(
                    cam.get("keys")
                        .and_then(Value::as_array)
                        .is_some_and(|k| !k.is_empty()),
                    "camera file {cam_rel} must have non-empty keys"
                );
            }
        }
    }

    for required in [
        "dense-world",
        "deep-zoom",
        "interacting-boundary",
        "perturbed-colony",
        "lineage-view",
        "raw-field-view",
    ] {
        assert!(
            categories.contains(required),
            "gallery/manifest.json missing required plan §2.4 category {required:?}"
        );
    }
}

/// Live `INV-09` Gate G5 verification: rendering the same world in an
/// aesthetic zoomed Glow look vs an honest Raw G5 look produces real non-empty
/// PNGs AND bit-for-bit identical committed `World::digest()`.
#[test]
fn paired_aesthetic_and_raw_gallery_renders_preserve_exact_world_digest() {
    let mut text = REFERENCE.to_owned();
    for (from, to) in [
        ("width = 128.0\n", "width = 32.0\n"),
        ("height = 128.0\n", "height = 32.0\n"),
        ("grid_width = 128\n", "grid_width = 32\n"),
        ("grid_height = 128\n", "grid_height = 32\n"),
        ("initial_count = 4096\n", "initial_count = 64\n"),
        ("maximum_count = 16384\n", "maximum_count = 256\n"),
        ("count = 8\n", "count = 4\n"),
        ("sigma_world_units = 10.0\n", "sigma_world_units = 3.0\n"),
    ] {
        text = text.replacen(from, to, 1);
    }
    let config = ValidatedConfig::from_toml(&text).expect("compact gallery config validates");
    let workers = NonZeroUsize::new(2).expect("2");

    let aesthetic_dir = scratch_dir("aesthetic");
    let raw_dir = scratch_dir("raw");

    let cam = CameraPath::from_json(
        r#"{"keys":[{"tick":0,"center":[16.0,16.0],"zoom":1.0},{"tick":8,"center":[16.0,16.0],"zoom":3.0}]}"#,
    )
    .expect("valid camera path");

    let run_render = |out_dir: PathBuf, style: Style, camera: Option<CameraPath>| -> String {
        let opts = RenderOptions {
            ticks: 8,
            every: 8,
            size: [128, 128],
            out: out_dir.clone(),
            cpu_workers: Some(workers),
            controllers: ControllerChoice::Cpu,
            render_threads: workers,
            video: None,
            fps: 30,
            style,
            camera,
        };
        let mut end_digest = String::new();
        let mut frame_paths = Vec::new();
        render(&config, &opts, |rec| match rec {
            RenderRecord::Frame { path, .. } => frame_paths.push(path.clone()),
            RenderRecord::End { digest, .. } => end_digest = digest.clone(),
            RenderRecord::Start { .. } => {}
        })
        .expect("render succeeds");
        assert_eq!(
            frame_paths.len(),
            2,
            "expected tick 0 and tick 8 PNG frames"
        );
        for p in &frame_paths {
            let meta = fs::metadata(p).expect("PNG written");
            assert!(meta.len() > 100, "PNG frame {p} must be non-empty");
        }
        end_digest
    };

    let aesthetic_digest = run_render(
        aesthetic_dir,
        Style {
            mapping: ColorMapping::LineageEnergy,
            look: Look::Glow,
            particles: Particles::Points,
            field: true,
            field_mapping: FieldMapping::Composition,
        },
        Some(cam),
    );
    let raw_digest = run_render(
        raw_dir,
        Style {
            mapping: ColorMapping::Lineage,
            look: Look::Raw,
            particles: Particles::Points,
            field: true,
            field_mapping: FieldMapping::Energy,
        },
        None,
    );

    assert_eq!(
        aesthetic_digest, raw_digest,
        "INV-09 VIOLATION: aesthetic render and raw G5 render diverged in scientific state!"
    );
}
