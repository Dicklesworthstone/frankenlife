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
use flife_studio::memory::{
    DEVICE_BYTES_PER_PIXEL, RESIZABLE_WINDOW_TARGET, admit, admit_kept_at, admit_resize,
    observer_bytes, observer_bytes_at, observer_bytes_for_resize, observer_limit,
    reserve_resize_headroom,
};
use flife_studio::{ControllerChoice, Exact, REPLAY_SLOTS, TimelineRecord, peak_observer_bytes};

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
fn recurrent_studio_auto_admits_batched_within_budget_and_refuses_tight_budget() {
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

    // V2-12 unlocked batched Metal & FrankenTorch execution for recurrent
    // worlds: admit all controller choices before installing a backend (since
    // installing a batched backend reserves scratch inside `world.footprint()`),
    // then verify each route installs and steps cleanly within FL-043 peak memory.
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
    let cpu = admit_kept_at(
        &world,
        ControllerChoice::Cpu,
        &founding,
        &view,
        [1280, 800],
        2.0,
        0,
    )
    .expect("recurrent Cpu admits");
    let torch = admit_kept_at(
        &world,
        ControllerChoice::Torch,
        &founding,
        &view,
        [1280, 800],
        2.0,
        0,
    )
    .expect("recurrent Torch admits");
    let metal = admit_kept_at(
        &world,
        ControllerChoice::Metal,
        &founding,
        &view,
        [1280, 800],
        2.0,
        0,
    )
    .expect("recurrent Metal admits");

    assert!(auto.backend > 0);
    assert_eq!(auto.backend, auto.budget_bytes - auto.fixed_bytes());
    assert_eq!(observer_limit(&auto), auto.render);
    assert_eq!(torch.backend, auto.backend);
    assert_eq!(metal.backend, auto.backend);
    assert_eq!(cpu.backend, 0);
    assert!(observer_limit(&cpu) > cpu.render);

    flife_cli::run::install(&mut world, ControllerChoice::Auto, auto.backend)
        .expect("recurrent Auto installs batched backend");
    assert!(
        world.controller_report().backend.is_some(),
        "V2-12 Auto on 1-worker recurrent world must install Metal or Torch"
    );
    world.step().expect("recurrent Auto tick");
    assert!(world.controller_report().batched > 0);
    assert_eq!(world.controller_report().deferred, 0);

    // Explicit Cpu switches back to CPU reference cleanly.
    flife_cli::run::install(&mut world, ControllerChoice::Cpu, cpu.backend)
        .expect("recurrent Cpu installs reference");
    assert!(world.controller_report().backend.is_none());

    // Explicit Torch and Metal execute recurrent batches cleanly (Metal may
    // report MissingCapability on hosts without a Metal device).
    flife_cli::run::install(&mut world, ControllerChoice::Torch, torch.backend)
        .expect("recurrent Torch installs");
    assert!(world.controller_report().backend.is_some());
    let before_torch = world.controller_report().batched;
    world.step().expect("recurrent Torch tick");
    assert!(world.controller_report().batched > before_torch);
    assert_eq!(world.controller_report().deferred, 0);

    match flife_cli::run::install(&mut world, ControllerChoice::Metal, metal.backend) {
        Ok(()) => {
            assert!(world.controller_report().backend.is_some());
            let before_metal = world.controller_report().batched;
            world.step().expect("recurrent Metal tick");
            assert!(world.controller_report().batched > before_metal);
            assert_eq!(world.controller_report().deferred, 0);
        }
        Err(CoreError::MissingCapability { .. }) => {}
        Err(other) => panic!("unexpected recurrent Metal install error: {other:?}"),
    }

    // Tight budget below the batched fixed footprint refuses Auto, Torch, and
    // Metal honestly before opening a session, while Cpu still fits when its
    // smaller unbatched footprint stays under the cap.
    let tight_limit = auto.fixed_bytes().saturating_sub(1);
    let tight_toml = recurrent_toml
        .lines()
        .map(|line| {
            if line.split('=').next().map(str::trim) == Some("maximum_total_bytes") {
                format!("maximum_total_bytes = {tight_limit}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let tight_config = ValidatedConfig::from_toml(&tight_toml).expect("valid tight recurrent toml");
    let tight_world = found(&tight_config);
    let tight_founding = Epoch::standalone(&tight_world).expect("tight epoch");
    let mut tight_view =
        WorldView::for_epoch(&tight_founding, [640, 400], [0, 0, 0]).expect("tight view");
    tight_view.enable_overlay();

    for batched in [
        ControllerChoice::Auto,
        ControllerChoice::Torch,
        ControllerChoice::Metal,
    ] {
        match admit_kept_at(
            &tight_world,
            batched,
            &tight_founding,
            &tight_view,
            [1280, 800],
            2.0,
            0,
        ) {
            Err(CoreError::BudgetExhausted { budget, .. }) => {
                assert!(budget.starts_with("peak memory"), "{batched:?}: {budget}");
            }
            other => panic!("expected peak-memory refusal for {batched:?}, got {other:?}"),
        }
    }
    if cpu.fixed_bytes() <= tight_limit {
        let admitted_cpu = admit_kept_at(
            &tight_world,
            ControllerChoice::Cpu,
            &tight_founding,
            &tight_view,
            [1280, 800],
            2.0,
            0,
        )
        .expect("unbatched Cpu still fits tight budget");
        assert_eq!(admitted_cpu.backend, 0);
    }
}

#[test]
fn atomic_refit_surface_and_timeline_peak_admission_avoid_phantom_1x_retina_spike() {
    let config = preset_with(&[]);
    let world = found(&config);
    let founding = Epoch::standalone(&world).expect("an epoch");
    let mut view = WorldView::for_epoch(&founding, [1920, 1080], [0, 0, 0]).expect("a view");
    view.enable_overlay();

    // Invalid scale must leave the view completely unchanged even when drawable changed.
    let before = *view.viewport();
    assert!(view.refit_surface([3840, 2160], 0.0).is_err());
    assert!(view.refit_surface([3840, 2160], f64::NAN).is_err());
    assert_eq!(*view.viewport(), before);

    // Valid Retina `Resize` paired immediately with `Density` before `Draw`
    // must admit at `[3840, 2160] @ 2.0`, NOT the phantom `[3840, 2160] @ 1.0`
    // density-bin charge.
    let retina_records = [
        TimelineRecord::Resize {
            drawable: [3840, 2160],
        },
        TimelineRecord::Density {
            pixels_per_point: Exact(2.0),
        },
        TimelineRecord::Draw {
            frame: 1,
            tick: 0,
            wall_seconds: Exact(0.016),
        },
    ];
    let retina_peak = peak_observer_bytes(&world, &founding, &view, [1920, 1080], &retina_records)
        .expect("computes retina peak");
    let replay_feed_bytes = founding
        .reserved_bytes()
        .saturating_mul(REPLAY_SLOTS as u64);
    let expected_2x =
        observer_bytes_for_resize(replay_feed_bytes, &view, [3840, 2160], 2.0).expect("2x bytes");
    let phantom_1x =
        observer_bytes_for_resize(replay_feed_bytes, &view, [3840, 2160], 1.0).expect("1x bytes");
    assert!(
        phantom_1x > expected_2x,
        "4K @ 1x has 4x as many density bins as 4K @ 2x"
    );
    assert_eq!(
        retina_peak, expected_2x,
        "paired Resize + Density before Draw must not overcharge 1x density bins"
    );
    // Replay allocates `REPLAY_SLOTS` (4) epoch slots regardless of a smaller
    // `execution.render_queue_capacity` (3 in the preset), so peak_observer_bytes
    // must charge all 4 slots rather than undercharging by 1+ slots.
    let preset_2x =
        observer_bytes_at(&world, &founding, &view, [3840, 2160], 2.0).expect("preset 2x");
    assert_eq!(
        retina_peak - preset_2x,
        founding.reserved_bytes().saturating_mul(
            REPLAY_SLOTS as u64 - u64::from(config.config().execution.render_queue_capacity)
        ),
        "peak_observer_bytes must charge all REPLAY_SLOTS slots actually allocated by replay"
    );

    // Conversely, drawing a frame at 4K @ 1x BEFORE switching to 2x DOES charge 4K @ 1x.
    let drawn_at_1x_records = [
        TimelineRecord::Resize {
            drawable: [3840, 2160],
        },
        TimelineRecord::Draw {
            frame: 1,
            tick: 0,
            wall_seconds: Exact(0.016),
        },
        TimelineRecord::Density {
            pixels_per_point: Exact(2.0),
        },
    ];
    let drawn_1x_peak =
        peak_observer_bytes(&world, &founding, &view, [1920, 1080], &drawn_at_1x_records)
            .expect("computes 1x draw peak");
    assert_eq!(drawn_1x_peak, phantom_1x);

    // An overlay or camera record between Resize and Density also flushes
    // geometry in `replay`, so `peak_observer_bytes` admits that exact state.
    let overlay_between_records = [
        TimelineRecord::Resize {
            drawable: [3840, 2160],
        },
        TimelineRecord::Overlay {
            text: "tick 0".to_owned(),
            swatches: Vec::new(),
        },
        TimelineRecord::Density {
            pixels_per_point: Exact(2.0),
        },
    ];
    let overlay_between_peak = peak_observer_bytes(
        &world,
        &founding,
        &view,
        [1920, 1080],
        &overlay_between_records,
    )
    .expect("computes overlay flush peak");
    assert_eq!(overlay_between_peak, phantom_1x);

    // Invalid Resize / Density records fail immediately even if overwritten.
    let invalid_intermediate = [
        TimelineRecord::Resize {
            drawable: [0, 1080],
        },
        TimelineRecord::Resize {
            drawable: [1920, 1080],
        },
    ];
    assert!(matches!(
        peak_observer_bytes(
            &world,
            &founding,
            &view,
            [1920, 1080],
            &invalid_intermediate
        ),
        Err(CoreError::NumericalInvalid { .. })
    ));
    let invalid_scale = [
        TimelineRecord::Density {
            pixels_per_point: Exact(0.0),
        },
        TimelineRecord::Density {
            pixels_per_point: Exact(2.0),
        },
    ];
    assert!(matches!(
        peak_observer_bytes(&world, &founding, &view, [1920, 1080], &invalid_scale),
        Err(CoreError::NumericalInvalid { .. })
    ));

    // Atomic refit succeeds and updates both drawable and pixels_per_point together.
    assert_eq!(
        view.refit_surface([3840, 2160], 2.0).expect("refits"),
        (true, true)
    );
    assert_eq!(view.viewport().drawable(), [3840, 2160]);
    assert_eq!(
        view.viewport().pixels_per_point().to_bits(),
        2.0_f64.to_bits()
    );
    assert_eq!(
        view.refit_surface([3840, 2160], 2.0).expect("no-op refit"),
        (false, false)
    );
}

/// V2-16 (`INV-15`): Studio memory admission accounts honestly for optional
/// `Epoch` (`cell_states` 1 B/org, `polarities` 2 B/org) and `FieldTexture`
/// (`trail_energy` 4 B/texel, `vascular_conductance` 4 B/texel) buffers
/// multiplied by `render_queue_capacity`, and drops that delta to exactly zero
/// when `--disable v2-visual-mappings` is set.
#[test]
fn v2_visual_mappings_observer_bytes_account_honestly_and_drop_to_zero_when_disabled() {
    use flife_core::FeatureOverride;

    let mut v2_toml = PRESET.replacen(
        "model = \"field-ecology-v1\"",
        "model = \"field-ecology-v2\"",
        1,
    );
    v2_toml.push_str(
        "\n[contact]\ndiameter = 0.25\nstiffness = 0.5\nadhesion = 0.25\nadhesion_range = 2.0\n",
    );

    let enabled_cfg = ValidatedConfig::from_toml_with_overrides(
        &v2_toml,
        &[FeatureOverride::Enable("all-v2".to_owned())],
    )
    .expect("valid v2 all-features config");
    let disabled_cfg = ValidatedConfig::from_toml_with_overrides(
        &v2_toml,
        &[
            FeatureOverride::Enable("all-v2".to_owned()),
            FeatureOverride::Disable("v2-visual-mappings".to_owned()),
        ],
    )
    .expect("valid v2 without visual mappings config");

    let world_on = found(&enabled_cfg);
    let world_off = found(&disabled_cfg);
    let founding_on = Epoch::standalone(&world_on).expect("v2 epoch on");
    let founding_off = Epoch::standalone(&world_off).expect("v2 epoch off");
    assert!(founding_on.cell_states().is_some());
    assert!(founding_on.polarities().is_some());
    assert!(founding_on.field().trail_energy().is_some());
    assert!(founding_on.field().vascular_conductance().is_some());
    assert!(founding_off.cell_states().is_none());
    assert!(founding_off.polarities().is_none());
    assert!(founding_off.field().trail_energy().is_none());
    assert!(founding_off.field().vascular_conductance().is_none());

    let capacity = founding_on.capacity() as u64;
    let [tx, ty] = founding_on.field().texels().map(u64::from);
    let expected_slot_delta = capacity * (1 + 2) + (tx * ty) * (4 + 4);
    assert_eq!(
        founding_on.reserved_bytes() - founding_off.reserved_bytes(),
        expected_slot_delta
    );

    let mut view_on = WorldView::for_epoch(&founding_on, [1280, 720], [0, 0, 0]).expect("view on");
    let mut view_off =
        WorldView::for_epoch(&founding_off, [1280, 720], [0, 0, 0]).expect("view off");
    view_on.enable_overlay();
    view_off.enable_overlay();

    let slots = u64::from(enabled_cfg.config().execution.render_queue_capacity);
    let epoch_delta = slots * expected_slot_delta;
    let view_delta = (view_on.reserved_bytes() - view_off.reserved_bytes())
        + (view_on.frame_bytes() - view_off.frame_bytes())
        + 2 * (view_on.preparation_limit() - view_off.preparation_limit());
    assert!(epoch_delta > 0, "v2 epoch slots must charge optional views");
    assert!(
        view_delta > 0,
        "v2 WorldView must charge optional polarity apical-tick buffers"
    );
    let obs_on = observer_bytes(&world_on, &founding_on, &view_on, [2560, 1440]);
    let obs_off = observer_bytes(&world_off, &founding_off, &view_off, [2560, 1440]);
    assert_eq!(obs_on - obs_off, epoch_delta + view_delta);

    let admitted_on = admit(
        &world_on,
        ControllerChoice::Cpu,
        &founding_on,
        &view_on,
        [2560, 1440],
    )
    .expect("admits v2 on");
    let admitted_off = admit(
        &world_off,
        ControllerChoice::Cpu,
        &founding_off,
        &view_off,
        [2560, 1440],
    )
    .expect("admits v2 off");
    assert_eq!(
        admitted_on.render - admitted_off.render,
        epoch_delta + view_delta
    );
}

#[test]
fn resizable_window_headroom_admits_live_growth_up_to_4k_without_exceeding_budget() {
    let config = preset_with(&[]);
    let world = found(&config);
    let founding = Epoch::standalone(&world).expect("an epoch");
    let slots = u64::from(config.config().execution.render_queue_capacity);
    let feed_bytes = founding.reserved_bytes() * slots;
    let mut view = WorldView::for_epoch(&founding, [640, 400], [0, 0, 0]).expect("a view");
    view.enable_overlay();

    let mut memory = admit_kept_at(
        &world,
        ControllerChoice::Auto,
        &founding,
        &view,
        [1280, 800],
        2.0,
        0,
    )
    .expect("admits opening Retina window");
    let before_backend = memory.backend;
    assert_eq!(observer_limit(&memory), memory.render);

    reserve_resize_headroom(&mut memory, feed_bytes, &view, [1280, 800], 2.0);
    assert!(memory.backend < before_backend);
    assert!(memory.backend >= before_backend / 2);
    assert!(memory.peak_bytes <= memory.budget_bytes);

    let grown_800 = admit_resize(&memory, feed_bytes, &view, [1600, 900], 2.0)
        .expect("800x450 @ 2x fits in resize headroom");
    assert!(grown_800 > memory.render);
    let grown_qhd = admit_resize(&memory, feed_bytes, &view, [2560, 1440], 2.0)
        .expect("1280x720 @ 2x (QHD) fits in resize headroom");
    assert!(grown_qhd >= grown_800);
    assert!(
        memory
            .fixed_bytes()
            .saturating_sub(memory.render)
            .saturating_add(grown_qhd)
            .saturating_add(memory.backend)
            <= memory.budget_bytes
    );

    // With a roomier budget (2.5 GiB), resize headroom covers full 4K cleanly
    // while still refusing impossible sizes under INV-15.
    let roomy = preset_with(&[("maximum_total_bytes", (2560_u64) << 20)]);
    let roomy_world = found(&roomy);
    let roomy_epoch = Epoch::standalone(&roomy_world).expect("roomy epoch");
    let roomy_feed = roomy_epoch.reserved_bytes() * slots;
    let mut roomy_view =
        WorldView::for_epoch(&roomy_epoch, [640, 400], [0, 0, 0]).expect("roomy view");
    roomy_view.enable_overlay();
    let mut roomy_mem = admit_kept_at(
        &roomy_world,
        ControllerChoice::Auto,
        &roomy_epoch,
        &roomy_view,
        [1280, 800],
        2.0,
        0,
    )
    .expect("admits roomy window");
    reserve_resize_headroom(&mut roomy_mem, roomy_feed, &roomy_view, [1280, 800], 2.0);
    let grown_4k = admit_resize(
        &roomy_mem,
        roomy_feed,
        &roomy_view,
        RESIZABLE_WINDOW_TARGET,
        2.0,
    )
    .expect("4K fits in roomy resize headroom");
    assert!(grown_4k > roomy_mem.render);
    assert!(matches!(
        admit_resize(&roomy_mem, roomy_feed, &roomy_view, [16384, 16384], 2.0),
        Err(CoreError::BudgetExhausted { .. })
    ));
}
