//! FL-020 evidence: picking the organism under the pointer by identity
//! through a panned and zoomed view, validated against the world rather
//! than a storage slot; and the world view, which keeps its camera through
//! a resize, re-places the shown epoch for a new view without moving the
//! field's or density's display scale, and caps its dots on screen.

mod common;

use flife_core::CoreError;
use flife_core::ids::OrganismId;
use flife_engine::{Disposition, Intervention, Stepped};
use flife_manim::{
    DOT_RADIUS, DOT_RADIUS_MAX_POINTS, MAXIMUM_ZOOM, PICK_RADIUS_PIXELS, Particles,
    SELECTION_RING_COLOR, SELECTION_RING_GAP_POINTS, SELECTION_RING_SEGMENTS,
    SELECTION_RING_WIDTH_POINTS, Style, Viewport, WorldView, dot_radius, pick,
};
use fmn_render::ThreeDDraw;

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn view(extent: [f64; 2], drawable: [u32; 2], center: [f64; 2], zoom: f64) -> Viewport {
    Viewport::fit(extent, drawable)
        .expect("a viewport")
        .with_view(center, zoom)
        .expect("a view")
}

/// With the seam under the pointer, the nearest drawn organism is the one
/// just across it; out of reach there is none; among equals the lowest
/// identity wins; hidden organisms (left out of `drawn`) are never picked.
#[test]
fn a_pick_is_the_nearest_drawn_identity_through_a_panned_view() {
    let viewport = view([128.0, 128.0], [800, 800], [0.0, 64.0], 1.0);
    let drawn = [
        (1, [127.5, 64.0]), // 0.5 west of the seam: 3.125 px from the center
        (2, [0.6, 64.0]),   // 0.6 east: 3.75 px
        (3, [64.0, 64.0]),  // the window's far edge
    ];
    let center = [400.0, 400.0];
    assert_eq!(
        pick(&viewport, drawn, center, PICK_RADIUS_PIXELS),
        Some(OrganismId::new(1))
    );
    assert_eq!(
        pick(&viewport, drawn, [403.75, 400.0], 1.0),
        Some(OrganismId::new(2))
    );
    assert_eq!(pick(&viewport, drawn, center, 3.0), None);
    assert_eq!(
        pick(
            &viewport,
            drawn[1..].iter().copied(),
            center,
            PICK_RADIUS_PIXELS
        ),
        Some(OrganismId::new(2))
    );
    let twins = [(9, [0.1, 64.0]), (4, [0.1, 64.0])];
    assert_eq!(
        pick(&viewport, twins, center, PICK_RADIUS_PIXELS),
        Some(OrganismId::new(4))
    );
    assert_eq!(pick(&viewport, drawn, [f64::NAN, 400.0], 8.0), None);
    assert_eq!(pick(&viewport, drawn, center, f64::INFINITY), None);
    // Zoomed 64 times, 0.5 units is 200 px: nothing is within reach of the
    // center, but the pointer over organism 2 finds it.
    let deep = view([128.0, 128.0], [800, 800], [0.0, 64.0], 64.0);
    assert_eq!(pick(&deep, drawn, center, PICK_RADIUS_PIXELS), None);
    let over_two = deep.world_to_drawable([0.6, 64.0]);
    assert_eq!(
        pick(&deep, drawn, over_two, PICK_RADIUS_PIXELS),
        Some(OrganismId::new(2))
    );
}

/// An organism picked on an epoch is an identity. Once it is removed and a
/// new organism takes its storage slot, the world no longer has that
/// identity, and the slot's new occupant is another one: a host validating
/// the pick against the world shows nothing, never the new occupant.
#[test]
fn a_picked_identity_is_validated_against_the_world_not_its_slot() {
    let config = common::small();
    let mut world = common::found(&config);
    let epoch = common::epoch(&world);
    let viewport = view(epoch.extent(), [640, 640], [16.0, 16.0], 2.0);
    let row = 5;
    let id = epoch.ids()[row];
    let position = epoch.positions()[row].map(f64::from);
    let drawn = epoch
        .ids()
        .iter()
        .zip(epoch.positions())
        .map(|(&id, position)| (id, position.map(f64::from)));
    let at = viewport.world_to_drawable(position);
    assert_eq!(pick(&viewport, drawn, at, 0.5), Some(OrganismId::new(id)));
    let slot = world
        .population()
        .slot(OrganismId::new(id))
        .expect("a slot");

    let cell = [position[0].floor() as u32, position[1].floor() as u32];
    let report = match world
        .step_with(&[
            Intervention::RemoveOrganisms {
                center: position,
                radius: 0.01,
                disposition: Disposition::Remove,
            },
            Intervention::IntroduceOrganisms {
                cell,
                count: 1,
                energy: 2.0,
            },
        ])
        .expect("the tick runs")
    {
        Stepped::Published(report) => report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    };
    assert_eq!(report.interventions[0].removed, 1);
    assert_eq!(report.interventions[1].introduced, 1);
    assert!(
        world.population().get(OrganismId::new(id)).is_none(),
        "the picked organism is gone"
    );
    let occupant = world.population().slots()[slot].expect("the slot is taken again");
    assert_ne!(occupant, OrganismId::new(id));
    assert!(occupant.get() >= 48, "{occupant} is a new identity");
}

/// The view keeps its camera through a resize; a new view of the same
/// world and drawable is taken (and an equal one is not), one of another
/// drawable is refused; placing the shown epoch for a pan moves neither
/// display scale; at deep zoom the dots are capped on screen.
#[test]
fn the_world_view_keeps_its_camera_and_places_without_moving_scales() {
    let config = common::small();
    let mut world = common::found(&config);
    let early = common::epoch(&world);
    common::advance(&mut world, 40);
    let later = common::epoch(&world);
    // Show the brighter field, then the dimmer: the display scale then sits
    // above the dimmer field's brightest texel, where any update would
    // decay it further.
    let brightest = |epoch: &flife_render::Lease| {
        epoch
            .field()
            .values()
            .iter()
            .copied()
            .filter(|value| value.is_finite())
            .fold(0.0_f32, f32::max)
    };
    let (bright, epoch) = if brightest(&early) > brightest(&later) {
        (early, later)
    } else {
        (later, early)
    };
    assert!(f64::from(brightest(&epoch)) < 0.9 * f64::from(brightest(&bright)));
    let mut view_ = WorldView::for_epoch(&epoch, [320, 240], [0, 0, 0]).expect("a view");
    view_.set_style(Style {
        particles: Particles::Density,
        ..Style::default()
    });
    view_.show(&bright).expect("show");
    view_.show(&epoch).expect("show");
    let (field, density) = (view_.field().scale(), view_.density().scale());
    assert!(
        field > f64::from(brightest(&epoch)),
        "the field scale is decaying"
    );
    assert!(density > 0.0);

    let panned = view_
        .viewport()
        .with_view([1.0, 30.0], 3.0)
        .expect("a view");
    assert!(view_.set_view(panned).expect("set"));
    assert!(!view_.set_view(panned).expect("set again"));
    for _ in 0..50 {
        view_.place(&epoch).expect("placed");
    }
    assert_eq!(
        view_.field().scale(),
        field,
        "placing moved the field scale"
    );
    assert_eq!(
        view_.density().scale(),
        density,
        "placing moved the density scale"
    );
    let Some(ThreeDDraw::Surface(layer)) = view_.density().draw() else {
        panic!("the density layer draws one surface");
    };
    // The window starts inside a bin on both axes (x = -15, y = 14 in a
    // 32-unit world): the quad still covers the window exactly, and its
    // texture coordinates start inside the bins' texture.
    let bins = view_.density().bins();
    assert_eq!(
        layer.mesh,
        &view_.viewport().periodic_quad(bins).expect("a quad")
    );
    assert!(
        layer
            .mesh
            .vertices()
            .iter()
            .all(|vertex| vertex.point[0].abs() == 16.0 && vertex.point[1].abs() == 16.0)
    );
    let start = layer.mesh.vertices()[0].uv;
    assert!(
        (0..2).all(|axis| (start[axis] * f64::from(bins[axis]) - 0.5).fract() != 0.0),
        "the window starts between corners: {start:?}"
    );

    assert!(view_.resize([640, 480]).expect("resized"));
    assert_eq!(view_.viewport().center(), [1.0, 30.0]);
    assert_eq!(view_.viewport().zoom(), 3.0);
    let other = Viewport::fit(epoch.extent(), [100, 100]).expect("a viewport");
    assert!(matches!(
        view_.set_view(other),
        Err(CoreError::LayoutMismatch { .. })
    ));

    // Points: at deep zoom the dot radius is the cap, in pixels.
    view_.set_style(Style::default());
    let deep = view_
        .viewport()
        .with_view([16.0, 16.0], MAXIMUM_ZOOM)
        .expect("a view");
    view_.set_view(deep).expect("set");
    view_.place(&epoch).expect("placed");
    let radius = dot_radius(view_.viewport());
    assert_eq!(radius * view_.viewport().scale(), DOT_RADIUS_MAX_POINTS);
    assert!(radius < DOT_RADIUS);
    let fitted = Viewport::fit(epoch.extent(), [640, 480]).expect("a viewport");
    assert_eq!(dot_radius(&fitted), DOT_RADIUS.max(1.5 / fitted.scale()));
    assert!(!view_.cloud().draws().is_empty());
}

/// The selection ring's vertices lie on its two circles around the scene
/// point of `position` in `viewport`: [`SELECTION_RING_GAP_POINTS`] and
/// that plus [`SELECTION_RING_WIDTH_POINTS`] outside the dot, in points at
/// the viewport's density.
fn assert_ring(mesh: &fmn_render::SurfaceMesh, viewport: &Viewport, position: [f64; 2]) {
    let pixel = viewport.pixels_per_point() / viewport.scale();
    let inner = dot_radius(viewport) + SELECTION_RING_GAP_POINTS * pixel;
    let outer = inner + SELECTION_RING_WIDTH_POINTS * pixel;
    let [x, y, _] = viewport.scene_point(position);
    let vertices = mesh.vertices();
    assert_eq!(vertices.len(), 2 * SELECTION_RING_SEGMENTS);
    for (at, vertex) in vertices.iter().enumerate() {
        let radius = (vertex.point[0] - x).hypot(vertex.point[1] - y);
        let expected = if at % 2 == 0 { outer } else { inner };
        assert!(
            (radius - expected).abs() <= 1e-9 * expected.max(1.0),
            "vertex {at} at radius {radius}, not {expected}"
        );
        assert_eq!(vertex.color, SELECTION_RING_COLOR);
    }
}

/// FL-020: the world view picks among the shown epoch's organisms and
/// selects by identity. The ring is the frame's last draw, around the
/// selected organism at any view; an identity the shown epoch lacks selects
/// nothing. Once a shown epoch no longer has the selected organism (removed,
/// its storage slot reused by an introduced one), the selection is dropped:
/// it never passes to the slot's new occupant. A living selection follows
/// its organism through new epochs.
#[test]
fn the_world_view_selects_by_identity_and_drops_a_dead_selection() {
    let config = common::small();
    let mut world = common::found(&config);
    let epoch = common::epoch(&world);
    let mut view_ = WorldView::for_epoch(&epoch, [640, 640], [0, 0, 0]).expect("a view");
    view_.show(&epoch).expect("show");
    assert!(
        view_
            .shown_rows()
            .map(|(id, _)| id)
            .eq(epoch.ids().iter().copied())
    );
    let row = 7;
    let id = OrganismId::new(epoch.ids()[row]);
    let position = epoch.positions()[row].map(f64::from);
    let viewport = *view_.viewport();
    let picked = view_.pick_in(&viewport, viewport.world_to_drawable(position), 0.5);
    assert_eq!(picked, Some(id));
    assert_eq!(view_.select(picked).expect("select"), Some(id));
    assert_eq!(view_.selected_position(), Some(position));
    let draws = view_.draws().expect("draws");
    let Some(ThreeDDraw::Surface(ring)) = draws.last() else {
        panic!("the ring is the last draw");
    };
    assert_ring(ring.mesh, &viewport, position);
    view_
        .job()
        .expect("FrankenManim takes the frame with its ring");
    let with_ring = draws.len();
    drop(draws);

    // Not in the shown epoch: nothing is selected.
    assert_eq!(
        view_
            .select(Some(OrganismId::new(1 << 40)))
            .expect("select"),
        None
    );
    assert!(view_.selection_draw().is_none());
    assert_eq!(view_.draws().expect("draws").len(), with_ring - 1);

    // Through a pan and a zoom the ring stays on the organism.
    view_.select(Some(id)).expect("select");
    let panned = viewport
        .with_view([position[0] + 3.0, position[1] - 1.0], 4.0)
        .expect("a view");
    view_.set_view(panned).expect("set");
    let Some(ThreeDDraw::Surface(ring)) = view_.selection_draw() else {
        panic!("a ring");
    };
    assert_ring(ring.mesh, &panned, position);

    // Removed, its slot taken by an introduced organism: the next shown
    // epoch drops the selection, and the old identity selects nothing.
    let slot = world.population().slot(id).expect("a slot");
    let cell = [position[0].floor() as u32, position[1].floor() as u32];
    match world
        .step_with(&[
            Intervention::RemoveOrganisms {
                center: position,
                radius: 0.01,
                disposition: Disposition::Remove,
            },
            Intervention::IntroduceOrganisms {
                cell,
                count: 1,
                energy: 2.0,
            },
        ])
        .expect("the tick runs")
    {
        Stepped::Published(report) => assert_eq!(report.interventions[0].removed, 1),
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
    let occupant = world.population().slots()[slot].expect("the slot is taken again");
    assert_ne!(occupant, id);
    let later = common::epoch(&world);
    assert!(later.ids().contains(&occupant.get()));
    view_.show(&later).expect("show");
    assert_eq!(view_.selected(), None);
    assert!(view_.selection_draw().is_none());
    assert_eq!(view_.select(Some(id)).expect("select"), None);

    // A living selection follows its organism.
    let living = OrganismId::new(later.ids()[0]);
    view_.select(Some(living)).expect("select");
    common::advance(&mut world, 1);
    let next = common::epoch(&world);
    let row = next
        .ids()
        .binary_search(&living.get())
        .expect("the organism lives a tick on");
    let moved = next.positions()[row].map(f64::from);
    view_.place(&next).expect("placed");
    assert_eq!(view_.selected(), Some(living));
    assert_eq!(view_.selected_position(), Some(moved));
    let Some(ThreeDDraw::Surface(ring)) = view_.selection_draw() else {
        panic!("a ring");
    };
    assert_ring(ring.mesh, view_.viewport(), moved);
}
