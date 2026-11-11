//! The particle cloud: one bounded scene object lowered from real render
//! epochs, in identity order, without allocating.

mod common;

use flife_core::CoreError;
use flife_core::ids::OrganismId;
use flife_manim::{
    DOT_RADIUS, DOT_RADIUS_MAX_POINTS, DOT_RADIUS_MIN_POINTS, LINEAGE_PALETTE, MAXIMUM_ZOOM,
    ParticleCloud, Viewport, WorldView, density_bins, dot_radius, dot_radius_at_least, hex_rgb,
    lineage_color, lineage_slot, selection_ring, selection_ring_around,
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
    assert_eq!(view.scene_objects(), 4);
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
    assert_eq!(small[0].radius, DOT_RADIUS_MIN_POINTS / 2.0);
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

/// Whether `view`'s selection ring is the one around a dot `radius` world
/// units across at `position`.
#[expect(
    clippy::panic,
    reason = "test helper: a missing ring is a test failure"
)]
fn ring_around(view: &WorldView, position: [f64; 2], radius: f64) -> Result<bool, CoreError> {
    let Some(ThreeDDraw::Surface(ring)) = view.selection_draw() else {
        panic!("a ring");
    };
    let around = selection_ring_around(view.viewport(), position, radius)?;
    Ok(ring.mesh.vertices() == around.vertices())
}

/// FL-019: `[presentation] point_radius_pixels` is the dot's smallest
/// radius on screen, in points like the window's size, in place of the
/// 1.5-point default. The shown dots, their halos and the selection ring
/// follow at once; a 2x screen doubles it in pixels; where the world radius
/// or the cap binds it changes nothing. A radius that is not positive and
/// finite, or is above the cap, is refused with the view unchanged.
#[test]
#[expect(clippy::float_cmp, reason = "radii are compared bit for bit")]
fn the_configured_point_radius_is_the_dots_smallest_on_screen() -> Result<(), CoreError> {
    let config = common::small();
    let world = common::found(&config);
    let epoch = common::epoch(&world);
    // 64 px over 32 world units: 2 px per unit, so the floor binds.
    let mut view = WorldView::for_epoch(&epoch, [64, 64], [0, 0, 0])?;
    view.show(&epoch)?;
    assert_eq!(view.point_radius(), DOT_RADIUS_MIN_POINTS);
    assert_eq!(
        dots(view.cloud().draws())[0].radius,
        DOT_RADIUS_MIN_POINTS / 2.0
    );
    let halos = summary(view.cloud().halos());
    let id = OrganismId::new(epoch.ids()[3]);
    assert_eq!(view.select(Some(id))?, Some(id));
    let position = view.selected_position().expect("drawn");

    // The preset's 0.8 points: 0.4 world units here, at once.
    view.set_point_radius(0.8)?;
    assert_eq!(view.point_radius(), 0.8);
    assert_eq!(view.cloud().min_radius_points(), 0.8);
    let fine = summary(view.cloud().draws());
    assert!(fine.iter().all(|dot| dot.1 == 0.8 / 2.0));
    let fine_halos = summary(view.cloud().halos());
    assert_eq!(fine_halos.len(), halos.len());
    for (before, after) in halos.iter().zip(&fine_halos) {
        assert_eq!((after.0, after.2), (before.0, before.2));
        let expected = before.1 * 0.8 / DOT_RADIUS_MIN_POINTS;
        assert!((after.1 - expected).abs() <= 1e-12 * expected, "{after:?}");
    }
    assert!(ring_around(&view, position, 0.8 / 2.0)?);
    assert!(!ring_around(&view, position, dot_radius(view.viewport()))?);
    // Showing the epoch again draws what the refit drew.
    view.show(&epoch)?;
    assert_eq!(summary(view.cloud().draws()), fine);
    assert!(ring_around(&view, position, 0.8 / 2.0)?, "reselected");
    for (shown, refit) in summary(view.cloud().halos()).iter().zip(&fine_halos) {
        assert!((shown.1 - refit.1).abs() <= 1e-12 * refit.1, "{shown:?}");
    }
    assert_eq!(
        dot_radius_at_least(view.viewport(), 0.8),
        0.8 / 2.0,
        "the free function is the view's"
    );
    assert_eq!(
        dot_radius_at_least(view.viewport(), DOT_RADIUS_MIN_POINTS),
        dot_radius(view.viewport())
    );

    // On a 2x screen twice as many pixels across, it is 1.6 drawable
    // pixels: the same world radius.
    let mut two = WorldView::for_epoch(&epoch, [128, 128], [0, 0, 0])?;
    assert!(two.set_pixels_per_point(2.0)?);
    two.set_point_radius(0.8)?;
    two.show(&epoch)?;
    assert_eq!(summary(two.cloud().draws()), fine);
    // A density change refits the selection ring to the floor too.
    assert!(view.set_pixels_per_point(2.0)?);
    assert!(ring_around(&view, position, 0.8)?);
    assert!(view.set_pixels_per_point(1.0)?);
    assert!(ring_around(&view, position, 0.8 / 2.0)?);

    // 640 px: 20 px per unit, so the world radius binds over 0.8 points; a
    // floor of 10 points binds over it.
    assert!(view.resize([640, 640])?);
    assert_eq!(dots(view.cloud().draws())[0].radius, DOT_RADIUS);
    view.set_point_radius(10.0)?;
    assert_eq!(dots(view.cloud().draws())[0].radius, 10.0 / 20.0);
    // Zoomed all the way in, the cap binds whatever the floor.
    let deep = view.viewport().with_view([16.0, 16.0], MAXIMUM_ZOOM)?;
    assert!(view.set_view(deep)?);
    view.place(&epoch)?;
    let capped = DOT_RADIUS_MAX_POINTS / (20.0 * MAXIMUM_ZOOM);
    assert_eq!(dots(view.cloud().draws())[0].radius, capped);
    view.set_point_radius(DOT_RADIUS_MAX_POINTS)?;
    assert_eq!(dots(view.cloud().draws())[0].radius, capped);
    view.set_point_radius(10.0)?;

    let before = summary(view.cloud().draws());
    for refused in [
        0.0,
        -0.8,
        f64::NAN,
        f64::INFINITY,
        DOT_RADIUS_MAX_POINTS + 0.5,
    ] {
        assert!(
            matches!(
                view.set_point_radius(refused),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "{refused}"
        );
        assert_eq!(view.point_radius(), 10.0);
        assert_eq!(summary(view.cloud().draws()), before);
    }
    let mut cloud = ParticleCloud::for_epoch(&epoch)?;
    assert!(cloud.set_min_radius_points(-1.0, view.viewport()).is_err());
    assert_eq!(cloud.min_radius_points(), DOT_RADIUS_MIN_POINTS);
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

/// FL-020: on a 2x screen the sizes given in points double in pixels, so a
/// view twice as many pixels across draws what the 1x view draws, in world
/// units: the floored and the capped dot and its halo, the selection ring
/// and the density bins. A density change refits what is shown, as a resize
/// does.
#[test]
fn sizes_in_points_look_the_same_on_a_2x_screen() -> Result<(), CoreError> {
    let config = common::small();
    let world = common::found(&config);
    let epoch = common::epoch(&world);
    let mut one = WorldView::for_epoch(&epoch, [64, 64], [0, 0, 0])?;
    let mut two = WorldView::for_epoch(&epoch, [128, 128], [0, 0, 0])?;
    one.show(&epoch)?;
    two.show(&epoch)?;
    assert_ne!(summary(two.cloud().draws()), summary(one.cloud().draws()));
    // The shown epoch is refitted at once, without showing it again.
    assert!(two.set_pixels_per_point(2.0)?);
    assert!(!two.set_pixels_per_point(2.0)?);
    // 2 px per world unit at 1x, 4 at 2x: the floor binds at both, 1.5
    // points, so 0.75 world units.
    assert_eq!(
        dots(one.cloud().draws())[0].radius,
        DOT_RADIUS_MIN_POINTS / 2.0
    );
    assert_eq!(summary(two.cloud().draws()), summary(one.cloud().draws()));
    assert_eq!(summary(two.cloud().halos()), summary(one.cloud().halos()));
    assert_eq!(two.density().bins(), one.density().bins());
    // Showing it again draws the same.
    two.show(&epoch)?;
    assert_eq!(summary(two.cloud().draws()), summary(one.cloud().draws()));
    let at = [3.0, 29.0];
    assert_eq!(
        selection_ring(two.viewport(), at)?.vertices(),
        selection_ring(one.viewport(), at)?.vertices()
    );
    assert_eq!(density_bins(two.viewport()), density_bins(one.viewport()));
    assert_eq!(two.density().bins(), one.density().bins());
    // Zoomed all the way in, the cap binds at both: 24 points, so 24 /
    // (2 * 4096) world units.
    for view in [&mut one, &mut two] {
        let deep = view.viewport().with_view([16.0, 16.0], MAXIMUM_ZOOM)?;
        assert!(view.set_view(deep)?);
        view.place(&epoch)?;
    }
    assert_eq!(
        dots(one.cloud().draws())[0].radius,
        DOT_RADIUS_MAX_POINTS / (2.0 * MAXIMUM_ZOOM)
    );
    assert_eq!(summary(two.cloud().draws()), summary(one.cloud().draws()));
    let fitted = two.viewport().refit();
    assert!(two.set_view(fitted)?);
    // Back to one pixel per point, the 2x drawable's floor is 1.5 px: the
    // world radius binds again.
    assert!(two.set_pixels_per_point(1.0)?);
    two.show(&epoch)?;
    assert_eq!(
        dots(two.cloud().draws())[0].radius,
        DOT_RADIUS.max(1.5 / 4.0)
    );
    assert!(matches!(
        two.set_pixels_per_point(f64::NAN),
        Err(CoreError::NumericalInvalid { .. })
    ));
    Ok(())
}
