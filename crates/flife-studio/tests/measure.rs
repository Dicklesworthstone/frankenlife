//! Plan §14.3's temporary measurement region in the Studio: the measuring
//! tool's disk is exactly the removal brush's, clicked or drawn, through
//! the view it was made on, and its preview shows what a click or a release
//! would measure, or the disk placed. The Studio's session thread answers a
//! measurement with what the committed world holds there, bit for bit what
//! the same world measured directly holds, and refuses a disk the world
//! cannot measure. Measuring changes nothing: the session ends on the
//! direct world's digest.
//!
//! No window opens: disks are made from viewports directly, and the
//! session thread is driven through its intent lane, as the Studio host
//! drives it after a measuring click.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::time::{Duration, Instant};

use flife_core::ids::SessionId;
use flife_core::{Tick, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, RegionMeasurement, Stepped, World};
use flife_manim::{Region, RegionEffect, RegionShape, Viewport};
use flife_studio::{
    BRUSH_ENERGY, Brush, Disk, Intent, MeasurePreview, SessionThread, Status, Stroke, Supplies,
    clicked, drawn, measure, stroke,
};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

const NONE: Supplies = Supplies {
    material: None,
    founder_energy: 2.0,
};

fn config() -> ValidatedConfig {
    ValidatedConfig::from_toml(PRESET).expect("the preset is valid")
}

fn found(config: &ValidatedConfig) -> World {
    World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds")
}

fn spawn(config: &ValidatedConfig) -> SessionThread {
    let (owner, handle) =
        flife_runtime::open(found(config), config, SessionId::new(1)).expect("the session opens");
    SessionThread::spawn(owner, handle).expect("the session thread starts")
}

/// Statuses until `done` holds for the ones gathered, within 60 seconds.
fn gather(
    session: &SessionThread,
    mut statuses: Vec<Status>,
    done: impl Fn(&[Status]) -> bool,
) -> Vec<Status> {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done(&statuses) {
        assert!(Instant::now() < deadline, "only {statuses:?}");
        statuses.extend(session.statuses());
        std::thread::sleep(Duration::from_millis(1));
    }
    statuses
}

/// Each measurement's seen tick and answer, in order.
fn measured(statuses: &[Status]) -> Vec<(Tick, RegionMeasurement)> {
    statuses
        .iter()
        .filter_map(|status| match status {
            Status::Measured { seen, measurement } => Some((*seen, *measurement)),
            _ => None,
        })
        .collect()
}

fn refusals(statuses: &[Status]) -> Vec<&'static str> {
    statuses
        .iter()
        .filter_map(|status| match status {
            Status::Refused { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect()
}

/// Bit for bit.
fn exactly(a: &RegionMeasurement, b: &RegionMeasurement) {
    assert_eq!(
        (a.tick, a.center.map(f64::to_bits), a.radius.to_bits()),
        (b.tick, b.center.map(f64::to_bits), b.radius.to_bits())
    );
    assert_eq!(
        (a.organisms, a.lineages, a.cells),
        (b.organisms, b.lineages, b.cells)
    );
    assert_eq!(a.organism_energy.to_bits(), b.organism_energy.to_bits());
    assert_eq!(a.field_energy.to_bits(), b.field_energy.to_bits());
}

/// A click measures the disk a click of the removal brush there removes,
/// and a drag the disk its drag draws, at every zoom; only a removal is a
/// disk. Off the world, in the letterbox margin or with no length, there
/// is none. Its outline is the removal's disc in the measurement colour.
#[test]
fn a_measured_disk_is_the_removal_brushs() {
    let extent = [32.0, 32.0];
    let logical = [800.0, 600.0];
    let fit = Viewport::fit(extent, [1600, 1200]).expect("fits");
    let at = [380.0, 290.0];
    for zoom in [1.0, 4.0, 13.5] {
        let viewport = fit.with_view([12.0, 9.0], zoom).expect("views");
        let removal =
            stroke(Brush::Removal, &viewport, logical, [32, 32], at, &NONE).expect("a removal");
        assert_eq!(clicked(&viewport, logical, at), Some(removal));
        let Stroke::Removal { center, radius } = removal else {
            panic!("{removal:?}");
        };
        let disk = measure::clicked(&viewport, logical, at).expect("a disk");
        assert_eq!(disk, Disk { center, radius }, "zoom {zoom}");
        assert_eq!(Disk::of(removal), Some(disk));
        assert_eq!(
            disk.region(),
            Region {
                shape: RegionShape::Disc { center, radius },
                effect: RegionEffect::Measures,
            }
        );
        for edge in [[at[0] + 30.0, at[1]], [at[0] - 4.0, at[1] + 41.0]] {
            let removal = drawn(&viewport, logical, at, edge).expect("a removal");
            assert_eq!(
                measure::drawn(&viewport, logical, at, edge),
                Disk::of(removal),
                "zoom {zoom}, {edge:?}"
            );
        }
    }
    for other in [
        Stroke::Energy {
            cell: [1, 2],
            amount: BRUSH_ENERGY,
        },
        Stroke::Introduction {
            cell: [1, 2],
            count: 3,
            energy: 2.0,
        },
    ] {
        assert_eq!(Disk::of(other), None);
    }
    let margin = [10.0, 300.0];
    assert_eq!(measure::clicked(&fit, logical, margin), None, "the margin");
    assert_eq!(
        measure::drawn(&fit, logical, margin, [60.0, 300.0]),
        None,
        "pressed in the margin"
    );
    assert_eq!(measure::drawn(&fit, logical, at, at), None, "no length");
}

/// While a disk is drawn the preview is what its release would measure,
/// whatever is placed; otherwise the disk placed, or with none what a
/// click at the pointer would measure. Without a content area or a
/// pointer, there is no disk to show but the placed one.
#[test]
fn the_preview_is_the_disk_a_gesture_would_measure_or_the_one_placed() {
    let logical = [800.0, 600.0];
    let viewport = Viewport::fit([32.0, 32.0], [1600, 1200]).expect("fits");
    let at = [380.0, 290.0];
    let pointer = [410.0, 330.0];
    let placed = measure::clicked(&viewport, logical, [500.0, 200.0]).expect("a disk");
    let release = measure::drawn(&viewport, logical, at, pointer);
    let click = measure::clicked(&viewport, logical, pointer);
    assert!(release.is_some() && click.is_some());
    let preview = |logical, pointer, drawing, placed| {
        measure::preview(&viewport, logical, pointer, drawing, placed)
    };
    for placed in [None, Some(placed)] {
        assert_eq!(
            preview(Some(logical), Some(pointer), Some(at), placed),
            MeasurePreview::Release(release)
        );
        assert_eq!(
            preview(None, Some(pointer), Some(at), placed),
            MeasurePreview::Release(None)
        );
        assert_eq!(
            preview(Some(logical), None, Some(at), placed),
            MeasurePreview::Release(None)
        );
    }
    assert_eq!(
        preview(Some(logical), Some(pointer), None, Some(placed)),
        MeasurePreview::Placed(placed)
    );
    assert_eq!(
        preview(None, None, None, Some(placed)),
        MeasurePreview::Placed(placed)
    );
    assert_eq!(
        preview(Some(logical), Some(pointer), None, None),
        MeasurePreview::Click(click)
    );
    assert_eq!(
        preview(Some(logical), Some([10.0, 300.0]), None, None),
        MeasurePreview::Click(None)
    );
    assert_eq!(
        preview(Some(logical), None, None, None),
        MeasurePreview::Click(None)
    );
    assert_eq!(MeasurePreview::Placed(placed).disk(), Some(placed));
    assert_eq!(MeasurePreview::Click(click).disk(), click);
    assert_eq!(MeasurePreview::Release(None).disk(), None);
}

/// A paused session answers each measurement at its committed boundary,
/// with the epoch it was seen on, exactly as the same world measured
/// directly; stepped, it answers for the new boundary. It refuses a disk
/// off the world or wider than half of it, and goes on answering. The
/// measurements change nothing: the session closes on the direct world.
#[test]
fn a_studio_session_measures_its_committed_world() {
    let config = config();
    let mut direct = found(&config);
    for _ in 0..3 {
        assert!(matches!(
            direct.step_with(&[]).expect("steps"),
            Stepped::Published(_)
        ));
    }
    let session = spawn(&config);
    for _ in 0..3 {
        session.send(Intent::Step).expect("offered");
    }
    let statuses = gather(&session, Vec::new(), |statuses| {
        statuses.contains(&Status::Stepped { tick: Tick::new(3) })
    });

    let target = direct.population().ids().nth(5).expect("an organism");
    let center = direct.population().get(target).expect("alive").position;
    let disks = [(center, 3.0), ([0.5, 0.5], 1.25), (center, 0.001)];
    for (center, radius) in disks {
        session
            .send(Intent::Measure {
                center,
                radius,
                seen: Tick::new(2),
            })
            .expect("offered");
    }
    let statuses = gather(&session, statuses, |statuses| {
        measured(statuses).len() == disks.len()
    });
    let answers = measured(&statuses);
    for ((center, radius), (seen, answer)) in disks.iter().zip(&answers) {
        assert_eq!(*seen, Tick::new(2));
        exactly(
            answer,
            &direct.measure_region(*center, *radius).expect("measures"),
        );
        assert_eq!(answer.tick, Tick::new(3));
    }
    assert!(answers[0].1.organisms >= 1, "the organism measured on");
    assert!(answers[0].1.field_energy > 0.0, "{:?}", answers[0].1);

    // Off the world, wider than half of it, or not a radius: refused, and
    // the session answers the next. The preset's world is 128 units square.
    let side = 128.0;
    let refused_before = refusals(&statuses).len();
    for (center, radius) in [
        ([-1.0, 4.0], 1.0),
        ([4.0, side], 1.0),
        (center, side / 2.0 + 1e-9),
        (center, f64::NAN),
        (center, 0.0),
    ] {
        session
            .send(Intent::Measure {
                center,
                radius,
                seen: Tick::new(3),
            })
            .expect("offered");
    }
    let statuses = gather(&session, statuses, |statuses| {
        refusals(statuses).len() == refused_before + 5
    });
    assert!(
        refusals(&statuses)[refused_before..]
            .iter()
            .all(|kind| *kind == "numerical-invalid"),
        "{statuses:?}"
    );

    // Stepped: the next boundary's answer.
    session.send(Intent::Step).expect("offered");
    session
        .send(Intent::Measure {
            center,
            radius: 3.0,
            seen: Tick::new(4),
        })
        .expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        measured(statuses).len() == disks.len() + 1
    });
    assert!(matches!(
        direct.step_with(&[]).expect("steps"),
        Stepped::Published(_)
    ));
    let (seen, answer) = *measured(&statuses).last().expect("an answer");
    assert_eq!(seen, Tick::new(4));
    exactly(
        &answer,
        &direct.measure_region(center, 3.0).expect("measures"),
    );
    assert_eq!(answer.tick, Tick::new(4));

    let closed = session.close().expect("closes");
    assert_eq!(closed.boundary.tick, Tick::new(4));
    assert_eq!(closed.world.digest(), direct.digest());
}
