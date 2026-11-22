//! FL-043: the Studio's admission, without a window. Its observers are its
//! feed, its view, one frame and its upload, and its device surfaces; the
//! backend gets what the budget leaves, and a budget the observers do not
//! fit is refused.

#![allow(
    clippy::print_stdout,
    clippy::cast_precision_loss,
    reason = "test: figures are printed"
)]

use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, World};
use flife_manim::WorldView;
use flife_render::Epoch;
use flife_studio::ControllerChoice;
use flife_studio::memory::{
    DEVICE_BYTES_PER_PIXEL, admit, admit_kept_at, admit_resize, observer_bytes,
    observer_bytes_for_resize, observer_limit,
};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn preset_with(pairs: &[(&str, u64)]) -> ValidatedConfig {
    let mut lines: Vec<String> = PRESET.lines().map(str::to_owned).collect();
    for &(key, value) in pairs {
        let line = lines
            .iter()
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .unwrap_or_else(|| panic!("{key} is in the preset"));
        lines[line] = format!("{key} = {value}");
    }
    ValidatedConfig::from_toml(&lines.join("\n")).expect("still valid")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn found(config: &ValidatedConfig) -> World {
    World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds")
}

#[test]
fn the_studio_is_admitted_with_its_observers_and_the_backend_gets_the_rest() {
    let config = preset_with(&[]);
    let world = found(&config);
    let founding = Epoch::standalone(&world).expect("an epoch");
    let mut view = WorldView::for_epoch(&founding, [1920, 1080], [0, 0, 0]).expect("a view");
    // At two pixels per point the window's drawable has four times the pixels.
    let drawable = [3840, 2160];
    let bare = observer_bytes(&world, &founding, &view, drawable);
    let (reserved, frame, preparation) = (
        view.reserved_bytes(),
        view.frame_bytes(),
        view.preparation_limit(),
    );
    // As `Studio::open` does: the overlay's text is admitted with the view
    // (FL-024).
    view.enable_overlay();
    let slots = u64::from(config.config().execution.render_queue_capacity);
    let tiles = (3840 / 16) * (2160 / 16);
    let observers = observer_bytes(&world, &founding, &view, drawable);
    assert_eq!(
        observers - bare,
        (view.reserved_bytes() - reserved)
            + (view.frame_bytes() - frame)
            + 2 * (view.preparation_limit() - preparation),
        "the overlay is charged"
    );
    println!(
        "studio overlay: {:.1} MiB",
        (observers - bare) as f64 / f64::from(1 << 20)
    );
    assert_eq!(
        observers,
        founding.reserved_bytes() * slots
            + view.reserved_bytes()
            + view.frame_bytes()
            + 2 * (view.preparation_limit() + view.texel_bytes())
            + 3840 * 2160 * DEVICE_BYTES_PER_PIXEL
            + tiles * 4
    );

    let torch =
        admit(&world, ControllerChoice::Torch, &founding, &view, drawable).expect("admitted");
    println!(
        "studio: observers {:.1} MiB, backend {:.1} MiB of {:.1} MiB",
        observers as f64 / f64::from(1 << 20),
        torch.backend as f64 / f64::from(1 << 20),
        torch.budget_bytes as f64 / f64::from(1 << 20),
    );
    assert_eq!(torch.render, observers);
    assert_eq!(torch.backend, torch.budget_bytes - torch.fixed_bytes());
    assert!(torch.backend > 0);
    let reference =
        admit(&world, ControllerChoice::Cpu, &founding, &view, drawable).expect("admitted");
    assert_eq!(reference.backend, 0);
    assert_eq!(reference.render, observers);

    // The preset's bloom (FL-019) keeps its row sums on the device, 16 bytes
    // a pixel of FrankenManim's widest ring, whatever the look: a band and
    // the widest blur's reach, 320 of the drawable's 2160 rows.
    let presentation = &config.config().presentation;
    assert!(presentation.bloom_enabled);
    let mut blooming = WorldView::for_epoch(&founding, [1920, 1080], [0, 0, 0]).expect("a view");
    blooming.enable_overlay();
    blooming
        .set_bloom(Some(presentation.bloom_strength))
        .expect("a bloom");
    assert_eq!(
        observer_bytes(&world, &founding, &blooming, drawable),
        observers + 3840 * 320 * 16
    );
    let bloomed = admit(
        &world,
        ControllerChoice::Torch,
        &founding,
        &blooming,
        drawable,
    )
    .expect("admitted");
    println!(
        "studio with the preset's bloom: observers {:.1} MiB, backend {:.1} MiB",
        bloomed.render as f64 / f64::from(1 << 20),
        bloomed.backend as f64 / f64::from(1 << 20),
    );
    assert_eq!(bloomed.backend, torch.backend - 3840 * 320 * 16);

    // A budget the observers do not fit is refused.
    let others = torch.fixed_bytes() - observers;
    let short = preset_with(&[("maximum_total_bytes", others + observers - 1)]);
    let world = found(&short);
    let founding = Epoch::standalone(&world).expect("an epoch");
    let mut view = WorldView::for_epoch(&founding, [1920, 1080], [0, 0, 0]).expect("a view");
    view.enable_overlay();
    match admit(&world, ControllerChoice::Torch, &founding, &view, drawable) {
        Err(CoreError::BudgetExhausted { budget, .. }) => {
            assert!(budget.starts_with("peak memory"), "{budget}");
        }
        other => panic!("expected a peak-memory refusal, got {other:?}"),
    }
}

#[test]
fn live_resize_is_admitted_within_headroom_and_refused_past_maximum_total_bytes() {
    let config = preset_with(&[]);
    let world = found(&config);
    let founding = Epoch::standalone(&world).expect("an epoch");
    let slots = u64::from(config.config().execution.render_queue_capacity);
    let feed_bytes = founding.reserved_bytes() * slots;
    let mut view = WorldView::for_epoch(&founding, [640, 400], [0, 0, 0]).expect("a view");
    view.enable_overlay();

    // Under CPU reference (`backend = 0`), remaining budget gives headroom to
    // grow cleanly to 1920x1080 @ 1x, while an oversized 16384x16384 surface
    // exceeds `maximum_total_bytes` and is refused.
    let cpu = admit(&world, ControllerChoice::Cpu, &founding, &view, [640, 400]).expect("admits");
    assert!(observer_limit(&cpu) > cpu.render);
    let grown = admit_resize(&cpu, feed_bytes, &view, [1920, 1080], 1.0).expect("1080p fits");
    assert!(grown > cpu.render);
    assert_eq!(
        grown,
        observer_bytes_for_resize(feed_bytes, &view, [1920, 1080], 1.0).expect("computes")
    );
    match admit_resize(&cpu, feed_bytes, &view, [16384, 16384], 1.0) {
        Err(CoreError::BudgetExhausted {
            budget,
            required,
            available,
        }) => {
            assert!(budget.contains("resize"), "{budget}");
            assert!(required > available);
            assert_eq!(available, observer_limit(&cpu));
        }
        other => panic!("expected resize BudgetExhausted, got {other:?}"),
    }

    // Under a batched backend (`backend = budget - fixed`), shrinking fits
    // while growing past the admitted ceiling is refused before allocation.
    let batched = admit(
        &world,
        ControllerChoice::Torch,
        &founding,
        &view,
        [640, 400],
    )
    .expect("admits");
    assert_eq!(observer_limit(&batched), batched.render);
    let shrunk = admit_resize(&batched, feed_bytes, &view, [320, 200], 1.0).expect("shrink fits");
    assert!(shrunk < batched.render);
    assert!(matches!(
        admit_resize(&batched, feed_bytes, &view, [3840, 2160], 2.0),
        Err(CoreError::BudgetExhausted { .. })
    ));
}

#[test]
fn opening_admission_matches_frame_one_at_fractional_and_os_resized_surfaces() {
    let config = preset_with(&[]);
    let world = found(&config);
    let founding = Epoch::standalone(&world).expect("an epoch");
    let slots = u64::from(config.config().execution.render_queue_capacity);
    let feed_bytes = founding.reserved_bytes() * slots;
    // View starts at logical `[640, 400] @ 1.0` (before frame 1 records
    // `Resize` and `Density`), while the host surface opens at an OS-resized
    // or fractional-scale drawable whose density bins exceed `[640, 400] @ 1.0`.
    let mut view = WorldView::for_epoch(&founding, [640, 400], [0, 0, 0]).expect("a view");
    view.enable_overlay();

    for (drawable, scale) in [([1280, 800], 2.0), ([1440, 900], 1.5), ([960, 600], 1.0)] {
        let admitted = admit_kept_at(
            &world,
            ControllerChoice::Torch,
            &founding,
            &view,
            drawable,
            scale,
            0,
        )
        .expect("admits opening surface");
        assert_eq!(observer_limit(&admitted), admitted.render);
        let frame_one = admit_resize(&admitted, feed_bytes, &view, drawable, scale)
            .expect("frame 1 must never refuse its own opening geometry");
        assert_eq!(frame_one, admitted.render);
        assert!(
            admit_resize(
                &admitted,
                feed_bytes,
                &view,
                [drawable[0] + 64, drawable[1] + 64],
                scale,
            )
            .is_err(),
            "growing past the admitted opening surface under full backend grant must be refused"
        );
    }
}

#[test]
fn recurrent_studio_auto_admits_cpu_without_batched_budget_and_refuses_explicit_batched() {
    let recurrent_toml = PRESET
        .replace(
            "family = \"independent-dense-mlp-v1\"",
            "family = \"independent-recurrent-mlp-v1\"\nrecurrent_state_width = 4",
        )
        .replace(
            "catalog = \"dense-104-36-catalog-v1\"",
            "catalog = \"recurrent-104-36-catalog-v1\"",
        );
    let config = ValidatedConfig::from_toml(&recurrent_toml).expect("valid recurrent config");
    let mut world = found(&config);
    assert_eq!(world.recurrent_state_width(), 4);
    let founding = Epoch::standalone(&world).expect("an epoch");
    let mut view = WorldView::for_epoch(&founding, [640, 400], [0, 0, 0]).expect("a view");
    view.enable_overlay();

    // Auto routes recurrent worlds to the CPU reference (`backend == 0`) so
    // Studio never starves observer headroom or installs an MLP-only backend.
    let auto = admit_kept_at(
        &world,
        ControllerChoice::Auto,
        &founding,
        &view,
        [1280, 800],
        2.0,
        0,
    )
    .expect("recurrent Auto admits");
    assert_eq!(auto.backend, 0);
    assert!(observer_limit(&auto) > auto.render);
    flife_cli::run::install(&mut world, ControllerChoice::Auto, auto.backend)
        .expect("recurrent Auto installs CPU reference");
    assert!(world.controller_report().backend.is_none());

    // Explicit batched choices refuse cleanly before opening a session.
    for explicit in [ControllerChoice::Torch, ControllerChoice::Metal] {
        let err = flife_cli::run::install(&mut world, explicit, 64 << 20)
            .expect_err("explicit batched choice on recurrent world must refuse");
        assert!(
            matches!(err, CoreError::MissingCapability { .. }),
            "expected MissingCapability for {explicit:?}, got {err:?}"
        );
    }
}
