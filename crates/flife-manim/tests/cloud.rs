//! The particle cloud: one bounded scene object lowered from real render
//! epochs, in identity order, without allocating.

mod common;

use flife_core::CoreError;
use flife_manim::{
    DOT_RADIUS, DOT_RADIUS_MIN_PIXELS, LINEAGE_PALETTE, ParticleCloud, Viewport, WorldView,
    dot_radius, hex_rgb, lineage_color, lineage_slot,
};
use fmn_render::ThreeDDraw;

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn fit(extent: [f64; 2], drawable: [u32; 2]) -> Viewport {
    Viewport::fit(extent, drawable).expect("a valid viewport")
}

#[expect(
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn dots(draws: &[ThreeDDraw<'static>]) -> Vec<fmn_render::TrueDotDraw> {
    draws
        .iter()
        .map(|draw| match draw {
            ThreeDDraw::TrueDot(dot) => *dot,
            other => panic!("not a true dot: {other:?}"),
        })
        .collect()
}

/// Each dot's centre, radius and colour, comparable.
fn summary(draws: &[ThreeDDraw<'static>]) -> Vec<([f64; 3], f64, [f64; 4])> {
    dots(draws)
        .iter()
        .map(|dot| {
            let color = dot.color;
            (dot.center, dot.radius, [color.r, color.g, color.b, color.a])
        })
        .collect()
}

#[test]
fn every_row_becomes_one_dot_in_identity_order() -> Result<(), CoreError> {
    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 24);
    let epoch = common::epoch(&world);
    let viewport = fit(epoch.extent(), [320, 320]);
    let mut cloud = ParticleCloud::for_epoch(&epoch)?;
    let draws = dots(cloud.update(&epoch, &viewport)?);
    assert_eq!(draws.len(), epoch.len());
    assert!(epoch.ids().windows(2).all(|pair| pair[0] < pair[1]));
    let radius = dot_radius(&viewport);
    for (row, dot) in draws.iter().enumerate() {
        let [x, y] = epoch.positions()[row];
        assert_eq!(
            dot.center,
            viewport.scene_point([f64::from(x), f64::from(y)]),
            "row {row}"
        );
        assert_eq!(dot.radius, radius);
        assert_eq!(dot.color, lineage_color(epoch.lineages()[row]));
    }
    Ok(())
}

#[test]
fn lowering_never_reallocates_and_the_view_is_one_scene_object() -> Result<(), CoreError> {
    let config = common::small();
    let mut world = common::found(&config);
    let first = common::epoch(&world);
    let mut view = WorldView::for_epoch(&first, [256, 256], [2, 3, 8])?;
    let reserved = view.cloud().reserved_bytes();
    let storage = view.cloud().draws().as_ptr();
    assert_eq!(view.cloud().capacity(), first.capacity());
    for _ in 0..6 {
        common::advance(&mut world, 8);
        let epoch = common::epoch(&world);
        view.show(&epoch)?;
        assert_eq!(view.cloud().draws().len(), epoch.len());
        assert_eq!(view.shown().map(|shown| shown.tick), Some(epoch.tick()));
        view.job()?;
    }
    view.resize([640, 360])?;
    assert_eq!(view.cloud().reserved_bytes(), reserved);
    assert_eq!(view.cloud().draws().as_ptr(), storage);
    assert_eq!(view.scene_objects(), 3);
    Ok(())
}

#[test]
fn a_population_past_the_ceiling_is_refused_and_keeps_what_was_shown() -> Result<(), CoreError> {
    let config = common::small();
    let world = common::found(&config);
    let epoch = common::epoch(&world);
    let viewport = fit(epoch.extent(), [128, 128]);
    let mut cloud = ParticleCloud::with_capacity(epoch.len() - 1)?;
    let refused = cloud.update(&epoch, &viewport);
    assert!(
        matches!(
            refused,
            Err(CoreError::BudgetExhausted {
                required,
                available,
                ..
            }) if required == epoch.len() as u64 && available == epoch.len() as u64 - 1
        ),
        "{refused:?}"
    );
    assert!(cloud.draws().is_empty());

    // An epoch of another world is refused too, and the draws stay.
    let mut fitting = ParticleCloud::for_epoch(&epoch)?;
    fitting.update(&epoch, &viewport)?;
    let before = summary(fitting.draws());
    let other = fit([64.0, 32.0], [128, 128]);
    let mismatched = fitting.update(&epoch, &other);
    assert!(
        matches!(mismatched, Err(CoreError::LayoutMismatch { .. })),
        "{mismatched:?}"
    );
    assert_eq!(summary(fitting.draws()), before);
    Ok(())
}

#[test]
fn a_shown_epoch_survives_a_refused_show_and_a_resize_refits_only_the_radius()
-> Result<(), CoreError> {
    let config = common::small();
    let world = common::found(&config);
    let epoch = common::epoch(&world);
    let mut view = WorldView::for_epoch(&epoch, [64, 64], [0, 0, 0])?;
    view.show(&epoch)?;
    let shown = view.shown();
    let small = dots(view.cloud().draws());
    // 64 px over 32 world units: 2 px per unit, so the pixel floor binds.
    assert_eq!(small[0].radius, DOT_RADIUS_MIN_PIXELS / 2.0);
    // 640 px: 20 px per unit, so the world radius binds.
    assert!(view.resize([640, 640])?);
    assert!(!view.resize([640, 640])?);
    let large = dots(view.cloud().draws());
    assert_eq!(large[0].radius, DOT_RADIUS);
    for (before, after) in small.iter().zip(&large) {
        assert_eq!(before.center, after.center);
        assert_eq!(before.color, after.color);
    }
    assert_eq!(view.shown(), shown);
    // A refused resize leaves the view as it was.
    assert!(view.resize([0, 640]).is_err());
    assert_eq!(view.viewport().drawable(), [640, 640]);
    Ok(())
}

#[test]
fn lineage_colours_are_fixed_and_spread_over_the_palette() {
    let slots: Vec<usize> = (1..=4096).map(lineage_slot).collect();
    for slot in 0..LINEAGE_PALETTE.len() {
        let count = slots.iter().filter(|&&used| used == slot).count();
        // 4096 / 12 is about 341; every colour is in use, none dominates.
        assert!((250..=450).contains(&count), "slot {slot}: {count}");
    }
    // Neighbouring identities are uncorrelated: about one pair in twelve
    // shares a colour, as for independent draws (4095 / 12 is about 341).
    let shared = slots.windows(2).filter(|pair| pair[0] == pair[1]).count();
    assert!(
        (250..=450).contains(&shared),
        "{shared} neighbouring pairs share a colour"
    );

    assert_eq!(lineage_slot(7), lineage_slot(7));
    assert_eq!(lineage_color(7), lineage_color(7));
}

#[test]
fn presentation_backgrounds_parse_as_rrggbb() {
    assert_eq!(hex_rgb("#020308"), Some([2, 3, 8]));
    assert_eq!(hex_rgb("#ffa0c1"), Some([0xFF, 0xA0, 0xC1]));
    for bad in [
        "020308",
        "#02030",
        "#0203080",
        "#zz0308",
        "#02\u{e9}308",
        "",
    ] {
        assert_eq!(hex_rgb(bad), None, "{bad:?}");
    }
}
