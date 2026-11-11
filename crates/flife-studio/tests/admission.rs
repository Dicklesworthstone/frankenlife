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
use flife_studio::memory::{DEVICE_BYTES_PER_PIXEL, admit, observer_bytes};

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
