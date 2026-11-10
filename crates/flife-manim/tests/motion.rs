//! FL-020 evidence (plan §13.5): the world view draws the display part of
//! the way from one committed epoch to the next, matched by identity. A
//! continuing organism is where `flife_render::interpolate` places it, a
//! birth appears only with its epoch, a death fades at its prior position,
//! an ambiguous gap snaps, and anything but an earlier state of the same
//! world is shown at rest. Picks and the selection follow what is drawn.
//!
//! The oracle for every drawn position is `interpolate` itself, which
//! `flife-render`'s tests check against each organism's tick-by-tick path.

mod common;

use flife_core::CoreError;
use flife_core::ids::OrganismId;
use flife_engine::{Disposition, Intervention, Stepped, World};
use flife_manim::{HALO_ALPHA, Motion, Style, WorldView, dot_radius, mapped_color, traits};
use flife_render::{Epoch, Gap, Interpolation, Lease, Prior, Publication, Publisher, interpolate};
use fmn_render::{ThreeDDraw, TrueDotDraw};

/// Two epochs of one world from one publisher: the world as it stands,
/// then after `change`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn pair(world: &mut World, change: impl FnOnce(&mut World)) -> (Publisher, Lease, Lease) {
    let mut publisher = Publisher::new(world, 3, || {}).expect("a publisher");
    let viewer = publisher.feed().viewer(2).expect("a viewer");
    let prior = viewer.latest().expect("the founding epoch");
    change(world);
    let published = publisher.offer(world).expect("an offer");
    assert!(
        matches!(published, Publication::Published { .. }),
        "{published:?}"
    );
    let current = viewer.latest().expect("the later epoch");
    assert!(current.id() > prior.id() && current.tick() > prior.tick());
    (publisher, prior, current)
}

/// One tick removing the organism nearest `row`'s position in a tiny disk
/// and introducing one: a death and a birth in the gap.
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn kill_and_introduce(world: &mut World, at: [f64; 2]) {
    let cell = [at[0].floor() as u32, at[1].floor() as u32];
    match world
        .step_with(&[
            Intervention::RemoveOrganisms {
                center: at,
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
        Stepped::Published(report) => {
            assert_eq!(report.interventions[0].removed, 1);
            assert_eq!(report.interventions[1].introduced, 1);
        }
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

#[expect(
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn dots<'a>(draws: &'a [ThreeDDraw<'static>]) -> Vec<&'a TrueDotDraw> {
    draws
        .iter()
        .map(|draw| match draw {
            ThreeDDraw::TrueDot(dot) => dot,
            other => panic!("not a dot: {other:?}"),
        })
        .collect()
}

fn prior_of(epoch: &Epoch) -> Prior {
    Prior {
        id: epoch.id(),
        tick: epoch.tick(),
    }
}

/// The dots and halos `view` draws, bit for bit.
#[expect(
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn drawn(view: &WorldView) -> Vec<[u64; 8]> {
    view.cloud()
        .halos()
        .iter()
        .chain(view.cloud().draws())
        .map(|draw| match draw {
            ThreeDDraw::TrueDot(dot) => [
                dot.center[0].to_bits(),
                dot.center[1].to_bits(),
                dot.center[2].to_bits(),
                dot.radius.to_bits(),
                dot.color.r.to_bits(),
                dot.color.g.to_bits(),
                dot.color.b.to_bits(),
                dot.color.a.to_bits(),
            ],
            other => panic!("not a dot: {other:?}"),
        })
        .collect()
}

#[test]
#[expect(clippy::float_cmp, reason = "test: drawn values are exact")]
fn the_display_moves_by_identity_shows_births_at_their_epoch_and_fades_the_dead() {
    let config = common::small();
    let mut world = common::found(&config);
    let victim_row = 5;
    let (victim, at) = {
        let founding = common::epoch(&world);
        (
            OrganismId::new(founding.ids()[victim_row]),
            founding.positions()[victim_row].map(f64::from),
        )
    };
    let (_publisher, prior, current) = pair(&mut world, |world| {
        kill_and_introduce(world, at);
        common::advance(world, 3);
    });
    let mut view = WorldView::for_epoch(&current, [640, 640], [0, 0, 0]).expect("a view");
    let reserved = view.cloud().reserved_bytes();
    view.show(&prior).expect("show the prior epoch");
    let viewport = *view.viewport();
    let style = Style::default();
    let radius = dot_radius(&viewport);
    let gap = Gap::between(&prior, &current).expect("a gap");
    assert_eq!(gap.ticks, 4);
    assert!(gap.unambiguous, "{gap:?}");
    let born: Vec<u64> = current
        .ids()
        .iter()
        .copied()
        .filter(|&id| prior.row(OrganismId::new(id)).is_none())
        .collect();
    assert!(
        !born.is_empty(),
        "the introduced organism is born in the gap"
    );
    assert!(
        current.row(victim).is_none(),
        "the removed organism is dead"
    );

    let mut oracle = Interpolation::default();
    for fraction in [0.0, 0.25, 0.5, 0.75] {
        let motion = view
            .show_between(Some(&prior), &current, fraction)
            .expect("shows between");
        interpolate(&prior, &current, fraction, &mut oracle).expect("the oracle");
        assert!(oracle.fading.contains(&victim_row));
        assert_eq!(
            motion,
            Some(Motion {
                prior: prior_of(&prior),
                fraction,
                gap,
                fading: oracle.fading.len(),
            })
        );
        assert_eq!(view.motion(), motion);
        assert_eq!(view.shown(), Some(prior_of(&current)));

        // The dead first, faded by 1 − fraction at their prior positions in
        // their prior colours; then every drawn row of the later epoch.
        let fade = 1.0 - fraction;
        let prior_traits: Vec<_> = traits(&prior).collect();
        let current_traits: Vec<_> = traits(&current).collect();
        let mut expected: Vec<([f64; 3], f64, [f64; 4])> = Vec::new();
        for &row in &oracle.fading {
            let color = mapped_color(style.mapping, &prior_traits[row], prior.maximum_speed());
            let center = viewport.scene_point(prior.positions()[row].map(f64::from));
            expected.push((center, radius, [color.r, color.g, color.b, color.a * fade]));
        }
        for (row, (&position, &shown)) in oracle.positions.iter().zip(&oracle.shown).enumerate() {
            let id = OrganismId::new(current.ids()[row]);
            assert_eq!(
                shown,
                !born.contains(&id.get()),
                "a birth waits for fraction 1"
            );
            if shown {
                let color =
                    mapped_color(style.mapping, &current_traits[row], current.maximum_speed());
                expected.push((
                    viewport.scene_point(position),
                    radius,
                    [color.r, color.g, color.b, color.a],
                ));
            }
            if fraction == 0.0
                && let Some(then) = prior.row(id)
            {
                assert_eq!(
                    position,
                    prior.positions()[then].map(f64::from),
                    "fraction 0 is the prior state"
                );
            }
        }
        let actual = dots(view.cloud().draws());
        assert_eq!(actual.len(), expected.len());
        assert_eq!(
            actual.len(),
            prior.len(),
            "the living and the dying are the prior rows"
        );
        for (dot, (center, radius, color)) in actual.iter().zip(&expected) {
            assert_eq!(dot.center, *center);
            assert_eq!(dot.radius, *radius);
            assert_eq!([dot.color.r, dot.color.g, dot.color.b, dot.color.a], *color);
        }
        let halos = dots(view.cloud().halos());
        assert_eq!(halos.len(), expected.len());
        let faded = oracle.fading.len();
        assert!(
            halos[..faded]
                .iter()
                .all(|halo| halo.color.a == HALO_ALPHA * fade)
        );
        assert!(halos[faded..].iter().all(|halo| halo.color.a == HALO_ALPHA));
        assert_eq!(view.cloud().reserved_bytes(), reserved, "nothing grew");

        // Picks and rows read what is drawn: never the dead, never the
        // unborn.
        let rows: Vec<(u64, [f64; 2])> = view.shown_rows().collect();
        let oracle_rows: Vec<(u64, [f64; 2])> = current
            .ids()
            .iter()
            .zip(oracle.positions.iter().zip(&oracle.shown))
            .filter(|(_, (_, shown))| **shown)
            .map(|(&id, (&position, _))| (id, position))
            .collect();
        assert_eq!(rows, oracle_rows);
        let dead_at = viewport.world_to_drawable(prior.positions()[victim_row].map(f64::from));
        assert_ne!(view.pick_in(&viewport, dead_at, 0.25), Some(victim));
        assert_eq!(
            view.select(Some(victim)).expect("select"),
            None,
            "the dead select nothing"
        );
        let newborn = OrganismId::new(born[0]);
        assert_eq!(
            view.select(Some(newborn)).expect("select"),
            None,
            "the unborn select nothing"
        );

        // A continuing organism is selected where it is drawn, and its ring
        // goes with it.
        let (id, position) = rows[victim_row.min(rows.len() - 1)];
        let id = OrganismId::new(id);
        assert_eq!(
            view.pick_in(&viewport, viewport.world_to_drawable(position), 0.25),
            Some(id)
        );
        assert_eq!(view.select(Some(id)).expect("select"), Some(id));
        assert_eq!(view.selected_position(), Some(position));
        view.select(None).expect("deselect");
    }

    // At fraction 1 the display is at rest on the later epoch, drawn bit
    // for bit as showing it alone draws it; the newborn is selectable.
    let motion = view
        .show_between(Some(&prior), &current, 1.0)
        .expect("shows at rest");
    assert_eq!(motion, None);
    assert_eq!(view.motion(), None);
    let mut alone = WorldView::for_epoch(&current, [640, 640], [0, 0, 0]).expect("a view");
    alone.show(&current).expect("show");
    assert_eq!(drawn(&view), drawn(&alone));
    let newborn = OrganismId::new(born[0]);
    assert_eq!(view.select(Some(newborn)).expect("select"), Some(newborn));
}

#[test]
fn advance_moves_only_the_organisms_and_follows_the_selection() {
    let config = common::small();
    let mut world = common::found(&config);
    let (_publisher, prior, current) = pair(&mut world, |world| common::advance(world, 2));
    let mut view = WorldView::for_epoch(&current, [640, 640], [0, 0, 0]).expect("a view");
    view.show_between(Some(&prior), &current, 0.0)
        .expect("shows between");
    let field = view.field().scale();
    let mesh: Vec<_> = format!("{:?}", view.field().draw()).into_bytes();
    let id = OrganismId::new(current.ids()[3]);
    assert_eq!(view.select(Some(id)).expect("select"), Some(id));
    let mut oracle = Interpolation::default();
    for fraction in [0.2, 0.6, 0.9] {
        let motion = view
            .advance(Some(&prior), &current, fraction)
            .expect("advances");
        assert_eq!(motion.map(|motion| motion.fraction), Some(fraction));
        interpolate(&prior, &current, fraction, &mut oracle).expect("the oracle");
        assert_eq!(view.selected(), Some(id));
        assert_eq!(view.selected_position(), Some(oracle.positions[3]));
        assert_eq!(view.field().scale().to_bits(), field.to_bits());
        assert_eq!(
            format!("{:?}", view.field().draw()).into_bytes(),
            mesh,
            "the field is not redrawn"
        );
    }
    assert_eq!(
        view.advance(Some(&prior), &current, 1.0).expect("rests"),
        None
    );
    assert_eq!(
        view.selected_position(),
        Some(current.positions()[3].map(f64::from))
    );

    // Only the shown epoch advances.
    let refused = view.advance(None, &prior, 0.5);
    assert!(
        matches!(refused, Err(CoreError::StateConflict { .. })),
        "{refused:?}"
    );
}

#[test]
fn an_ambiguous_gap_snaps_and_a_foreign_prior_is_shown_at_rest() {
    let config = common::small();
    let mut world = common::found(&config);
    // On a 32-unit world, 600 ticks can carry an organism past half of it.
    let (_publisher, prior, current) = pair(&mut world, |world| common::advance(world, 600));
    let gap = Gap::between(&prior, &current).expect("a gap");
    assert!(!gap.unambiguous, "{gap:?}");
    let mut view = WorldView::for_epoch(&current, [640, 640], [0, 0, 0]).expect("a view");
    view.show(&prior).expect("show");
    let motion = view
        .show_between(Some(&prior), &current, 0.5)
        .expect("shows between")
        .expect("a motion");
    assert!(!motion.gap.unambiguous);
    let mut continuing = 0;
    for (id, position) in view.shown_rows() {
        let row = current.row(OrganismId::new(id)).expect("a row");
        assert!(
            prior.row(OrganismId::new(id)).is_some(),
            "the unborn are not drawn"
        );
        assert_eq!(
            position,
            current.positions()[row].map(f64::from),
            "snapped, no path"
        );
        continuing += 1;
    }
    assert!(continuing > 0);

    // A prior that is not an earlier state of this world: at rest.
    let mut alone = WorldView::for_epoch(&current, [640, 640], [0, 0, 0]).expect("a view");
    alone.show(&prior).expect("show");
    for (stale, shown) in [(&current, &prior), (&prior, &prior)] {
        assert_eq!(
            view.show_between(Some(stale), shown, 0.5).expect("shows"),
            None
        );
        assert_eq!(drawn(&view), drawn(&alone));
    }
    let other = common::found(&common::config(&[
        ("world", "width", "64.0"),
        ("world", "height", "64.0"),
        ("world", "grid_width", "64"),
        ("world", "grid_height", "64"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "1024"),
    ]));
    let foreign = common::epoch(&other);
    let refused = view.show_between(Some(&foreign), &prior, 0.5);
    assert!(
        matches!(refused, Err(CoreError::LayoutMismatch { .. })),
        "{refused:?}"
    );

    // A fraction outside [0, 1] is refused, and the organisms stay.
    for fraction in [-0.1, 1.5, f64::NAN] {
        let refused = view.show_between(Some(&prior), &current, fraction);
        assert!(
            matches!(refused, Err(CoreError::NumericalInvalid { .. })),
            "{refused:?}"
        );
        assert_eq!(drawn(&view), drawn(&alone));
    }
}
