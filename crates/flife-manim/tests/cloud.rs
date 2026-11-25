//! The particle cloud: one bounded scene object lowered from real render
//! epochs, in identity order, without allocating.

mod common;

use flife_core::CoreError;
use flife_core::ids::OrganismId;
use flife_manim::{
    DOT_RADIUS, DOT_RADIUS_MAX_POINTS, DOT_RADIUS_MIN_POINTS, LINEAGE_PALETTE, Look, MAXIMUM_ZOOM,
    ParticleCloud, SPHERE_SHADING, Style, Viewport, WorldView, density_bins, dot_radius,
    dot_radius_at_least, dot_radius_with, hex_rgb, lineage_color, lineage_slot, selection_ring,
    selection_ring_around,
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
    assert_eq!(view.scene_objects(), 5);
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

/// FL-024's G5 review: past the 24-point cap a confluent world's touching
/// bodies were drawn at under half their size, as loose dots with gaps the
/// world does not have. A declared body keeps its world size up to an eighth
/// of the drawable's height; point organisms keep the cap.
#[test]
#[expect(clippy::float_cmp, reason = "radii are compared bit for bit")]
fn a_declared_body_keeps_its_size_past_the_point_cap() -> Result<(), CoreError> {
    let config = common::small();
    let world = common::found(&config);
    let epoch = common::epoch(&world);
    // 640 px over 32 world units: 20 px per unit at zoom 1.
    let mut view = WorldView::for_epoch(&epoch, [640, 640], [0, 0, 0])?;
    view.show(&epoch)?;
    let id = OrganismId::new(epoch.ids()[3]);
    assert_eq!(view.select(Some(id))?, Some(id));
    let middle = [320.0, 320.0];
    view.set_view(view.viewport().zoomed_about(middle, 10.0)?)?;
    view.show(&epoch)?;
    assert_eq!(view.viewport().scale(), 200.0);
    // A point organism stops at 24 px.
    assert!(
        dots(view.cloud().draws())
            .iter()
            .all(|dot| dot.radius == DOT_RADIUS_MAX_POINTS / 200.0)
    );
    assert!(!view.cloud().draws().is_empty());

    // A body of 0.2 units is 40 px: drawn at its size, past the 24 px cap,
    // and so is the selection ring around it.
    view.set_body_radius(0.2)?;
    assert!(
        dots(view.cloud().draws())
            .iter()
            .all(|dot| dot.radius == 0.2)
    );
    let selected = view.selected_position().expect("drawn");
    assert!(ring_around(&view, selected, 0.2)?);
    view.show(&epoch)?;
    assert!(
        dots(view.cloud().draws())
            .iter()
            .all(|dot| dot.radius == 0.2)
    );

    // At 32x (640 px per unit) the body would be 128 px: it stops at an
    // eighth of the 640 px drawable, 80 px.
    view.set_view(view.viewport().zoomed_about(middle, 3.2)?)?;
    view.show(&epoch)?;
    let scale = view.viewport().scale();
    assert!((scale - 640.0).abs() < 1e-9, "{scale}");
    assert!(
        dots(view.cloud().draws())
            .iter()
            .all(|dot| dot.radius == 80.0 / scale)
    );

    // The free rule: point organisms are unchanged, and a drawable too small
    // for an eighth to reach 24 px keeps the 24 px cap for bodies too.
    let fitted = fit([32.0, 32.0], [640, 640]).zoomed_about(middle, 10.0)?;
    assert_eq!(
        dot_radius_with(&fitted, DOT_RADIUS_MIN_POINTS, None),
        dot_radius(&fitted)
    );
    assert_eq!(
        dot_radius_with(&fitted, DOT_RADIUS_MIN_POINTS, Some(0.2)),
        0.2
    );
    let tiny = fit([32.0, 32.0], [64, 64]).zoomed_about([32.0, 32.0], 100.0)?;
    assert_eq!(tiny.scale(), 200.0);
    assert_eq!(
        dot_radius_with(&tiny, DOT_RADIUS_MIN_POINTS, Some(0.2)),
        DOT_RADIUS_MAX_POINTS / 200.0
    );
    // A body smaller than the floor still takes the floor.
    assert_eq!(
        dot_radius_with(&fitted, DOT_RADIUS_MIN_POINTS, Some(0.001)),
        DOT_RADIUS_MIN_POINTS / 200.0
    );
    Ok(())
}

#[test]
#[expect(clippy::float_cmp, reason = "radii are compared bit for bit")]
fn a_body_radius_sizes_every_dot_and_the_sphere_look_lights_them() -> Result<(), CoreError> {
    let config = common::small();
    let world = common::found(&config);
    let epoch = common::epoch(&world);
    // 640 px over 32 world units: 20 px per unit, so the world radius binds.
    let mut view = WorldView::for_epoch(&epoch, [640, 640], [0, 0, 0])?;
    view.show(&epoch)?;
    assert_eq!(dots(view.cloud().draws())[0].radius, DOT_RADIUS);
    let id = OrganismId::new(epoch.ids()[3]);
    assert_eq!(view.select(Some(id))?, Some(id));
    let position = view.selected_position().expect("drawn");

    // A contact model's body: half its diameter, at once and on every show.
    view.set_body_radius(0.2)?;
    assert!(
        dots(view.cloud().draws())
            .iter()
            .all(|dot| dot.radius == 0.2)
    );
    assert!(ring_around(&view, position, 0.2)?);
    view.show(&epoch)?;
    assert!(
        dots(view.cloud().draws())
            .iter()
            .all(|dot| dot.radius == 0.2)
    );
    // The on-screen floor still binds a body smaller than it.
    view.set_body_radius(0.01)?;
    assert_eq!(
        dots(view.cloud().draws())[0].radius,
        DOT_RADIUS_MIN_POINTS / 20.0
    );
    for refused in [0.0, -0.1, f64::NAN, f64::INFINITY] {
        assert!(view.set_body_radius(refused).is_err(), "{refused}");
    }
    view.set_body_radius(0.2)?;

    // Raw dots are unlit; the sphere look lights each one, and only that.
    assert!(
        dots(view.cloud().draws())
            .iter()
            .all(|dot| dot.shading == [0.0; 3])
    );
    let flat = summary(view.cloud().draws());
    view.set_style(Style {
        look: Look::Sphere,
        ..view.style()
    });
    view.show(&epoch)?;
    let lit = dots(view.cloud().draws());
    assert!(lit.iter().all(|dot| dot.shading == SPHERE_SHADING));
    assert_eq!(
        summary(view.cloud().draws()),
        flat,
        "same centres, radii, colours"
    );
    assert!(view.cloud().halos().is_empty(), "no halos");
    assert!(view.frame_bloom().is_none(), "no bloom");
    Ok(())
}

/// FL-033 bug regression: `ParticleCloud::update_between` must emit every
/// fading row even if `between.fading` is not sorted ascending, `set_body_radius`
/// must ignore non-positive/non-finite radii, and zero-population (extinct)
/// worlds must render cleanly with and without the field.
#[test]
fn update_between_handles_out_of_order_fading_and_zero_population() -> Result<(), CoreError> {
    use flife_engine::{Disposition, Intervention, Stepped};
    use flife_render::Interpolation;

    let config = common::small();
    let mut world = common::found(&config);
    let prior = common::epoch(&world);
    // Wipe out the population to produce a real extinct epoch (0 organisms):
    // four discs of radius `ext[0]/2` at the quarter-torus grid cover every
    // point on the torus (max distance √2 · L/4 < L/2) within the legal radius cap.
    let ext = prior.extent();
    let r = ext[0].min(ext[1]) / 2.0;
    let [hx, hy] = [ext[0] / 2.0, ext[1] / 2.0];
    match world.step_with(&[
        Intervention::RemoveOrganisms {
            center: [0.0, 0.0],
            radius: r,
            disposition: Disposition::Remove,
        },
        Intervention::RemoveOrganisms {
            center: [hx, 0.0],
            radius: r,
            disposition: Disposition::Remove,
        },
        Intervention::RemoveOrganisms {
            center: [0.0, hy],
            radius: r,
            disposition: Disposition::Remove,
        },
        Intervention::RemoveOrganisms {
            center: [hx, hy],
            radius: r,
            disposition: Disposition::Remove,
        },
    ])? {
        Stepped::Published(_) => {}
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
    let empty = common::epoch(&world);
    assert_eq!(empty.len(), 0, "extinct epoch");

    let viewport = fit(prior.extent(), [128, 128]);
    let mut cloud = ParticleCloud::for_epoch(&prior)?;
    // Out-of-order fading rows: the old peekable scan silently dropped 2 and 9.
    let between = Interpolation {
        positions: Vec::new(),
        shown: Vec::new(),
        fading: vec![15, 2, 9],
        snapped: false,
    };
    let draws = dots(cloud.update_between(
        Some(&prior),
        &empty,
        &between,
        0.25,
        &viewport,
        &Style::default(),
    )?);
    assert_eq!(draws.len(), 3, "every fading row must be emitted");
    assert_eq!(cloud.halos().len(), 3);
    for (dot, &row) in draws.iter().zip(&between.fading) {
        let [x, y] = prior.positions()[row];
        assert_eq!(
            dot.center,
            viewport.scene_point([f64::from(x), f64::from(y)])
        );
        assert_eq!(dot.color.a, 0.75);
    }

    // set_body_radius ignores invalid values instead of corrupting self.body.
    let valid_body = cloud.body_radius();
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        cloud.set_body_radius(bad, &viewport);
        assert_eq!(cloud.body_radius(), valid_body, "{bad}");
    }

    // Zero-population WorldView renders cleanly in every style (including 0 draws).
    for style in [
        Style::default(),
        Style {
            field: false,
            ..Style::default()
        },
        Style {
            look: Look::Raw,
            field: false,
            ..Style::default()
        },
        Style {
            look: Look::Sphere,
            field: false,
            ..Style::default()
        },
        Style {
            particles: flife_manim::Particles::Density,
            ..Style::default()
        },
        Style {
            particles: flife_manim::Particles::Density,
            look: Look::Raw,
            ..Style::default()
        },
    ] {
        let mut view = WorldView::for_epoch(&empty, [64, 64], [10, 20, 30])?;
        view.set_style(style);
        view.show(&empty)?;
        assert_eq!(view.cloud().draws().len(), 0);
        let rgba = view.render_rgba8(1)?;
        assert_eq!(rgba.len(), 64 * 64 * 4);
    }
    Ok(())
}
