//! FL-022 evidence (plan §14.3, §12.3): a brush click becomes a world-space
//! command payload through the view it was made on, and the Studio's
//! session thread submits it for the next open boundary, reports where it
//! will apply, then reports the tick it applied at and the energy it
//! moved. Those are what a world stepped directly with the same
//! intervention at the same tick gives, bit for bit, and the session ends
//! on that world's digest.
//!
//! No window opens: strokes are made from viewports directly, and the
//! session thread is driven through its intent lane, as the Studio host
//! drives it after a brush click.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "test helpers: a failure in one is a test failure; exact equality is the claim"
)]

use std::time::{Duration, Instant};

use flife_core::ids::{CommandId, SessionId};
use flife_core::{Tick, ValidatedConfig};
use flife_engine::{
    AppliedIntervention, Arrangement, Disposition, Intervention, NumericalProfile, Stepped, World,
};
use flife_manim::Viewport;
use flife_runtime::Outcome;
use flife_studio::{
    BRUSH_ENERGY, BRUSH_RADIUS_POINTS, Brush, Intent, OUTCOME_POLL, SessionThread, Status, Stroke,
    stroke,
};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// The preset at 32 × 32 world units on a 32 × 32 grid with 96 founders.
fn small() -> ValidatedConfig {
    let mut lines: Vec<String> = PRESET.lines().map(str::to_owned).collect();
    for (section, key, value) in [
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "96"),
        ("population", "maximum_count", "1024"),
    ] {
        let header = format!("[{section}]");
        let at = lines
            .iter()
            .position(|line| line.trim() == header)
            .unwrap_or_else(|| panic!("no section {header}"));
        let line = lines[at + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| at + 1 + offset)
            .unwrap_or_else(|| panic!("no key {key} in {header}"));
        lines[line] = format!("{key} = {value}");
    }
    ValidatedConfig::from_toml(&lines.join("\n")).expect("a valid configuration")
}

fn found(config: &ValidatedConfig) -> World {
    World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds")
}

/// One published tick with `interventions` at its boundary, and what they
/// applied.
fn step_with(world: &mut World, interventions: &[Intervention]) -> Vec<AppliedIntervention> {
    match world.step_with(interventions).expect("steps") {
        Stepped::Published(report) => report.interventions,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
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

fn count(statuses: &[Status], which: fn(&Status) -> bool) -> usize {
    statuses.iter().filter(|status| which(status)).count()
}

fn is_submitted(status: &Status) -> bool {
    matches!(status, Status::Submitted { .. })
}

fn is_finished(status: &Status) -> bool {
    matches!(status, Status::Finished { .. })
}

/// Each submission: its command, stroke, seen tick and effective tick.
fn submissions(statuses: &[Status]) -> Vec<(CommandId, Stroke, Tick, Tick)> {
    statuses
        .iter()
        .filter_map(|status| match status {
            Status::Submitted {
                command,
                stroke,
                seen,
                effective,
            } => Some((*command, *stroke, *seen, *effective)),
            _ => None,
        })
        .collect()
}

/// Each applied command: its command, applied tick, published tick and
/// energy. Any other outcome fails.
fn applied(statuses: &[Status]) -> Vec<(CommandId, Tick, Tick, f64)> {
    let mut applied: Vec<_> = statuses
        .iter()
        .filter_map(|status| match status {
            Status::Finished { command, outcome } => match outcome {
                Outcome::Applied {
                    tick,
                    published,
                    applied,
                    ..
                } => Some((*command, *tick, *published, *applied)),
                other => panic!("{command:?} finished as {other:?}"),
            },
            _ => None,
        })
        .collect();
    applied.sort_by_key(|(command, ..)| *command);
    applied
}

/// A logical point, the world point under it and the stroke made there.
type Click = ([f64; 2], Option<[f64; 2]>, Option<Stroke>);

/// The world point under each logical point of a lattice over the content
/// area, with the energy stroke there.
fn lattice(viewport: &Viewport, logical: [f64; 2], cells: [u32; 2]) -> Vec<Click> {
    let mut made = Vec::new();
    let mut y = 0.25;
    while y < logical[1] {
        let mut x = 0.4;
        while x < logical[0] {
            let at = [x, y];
            made.push((
                at,
                viewport.logical_to_world(at, logical),
                stroke(Brush::Energy, viewport, logical, cells, at),
            ));
            x += 3.7;
        }
        y += 4.3;
    }
    made
}

/// The energy brush names the cell whose square holds the world point
/// under the click — at fit, zoomed and panned across the seams, on a grid
/// whose cells are not square — and nothing in the letterbox margin.
#[test]
fn an_energy_click_names_the_cell_under_it() {
    let extent = [32.0, 32.0];
    let cells = [64, 16];
    let width = [0.5, 2.0];
    // A 2× drawable, letterboxed left and right at fit.
    let logical = [800.0, 600.0];
    let fit = Viewport::fit(extent, [1600, 1200]).expect("fits");
    let zoomed = fit.with_view([3.3, 30.1], 2.75).expect("views");
    let seam = fit.with_view([0.0, 0.0], 2.0).expect("views");
    let mut margins = 0;
    let mut seen = [[false; 2]; 2];
    for viewport in [fit, zoomed, seam] {
        for (at, world, made) in lattice(&viewport, logical, cells) {
            let Some(world) = world else {
                assert_eq!(made, None, "{at:?}");
                margins += 1;
                continue;
            };
            let Some(Stroke::Energy { cell, amount }) = made else {
                panic!("{at:?} made {made:?}");
            };
            assert_eq!(amount, BRUSH_ENERGY);
            for axis in 0..2 {
                let low = f64::from(cell[axis]) * width[axis];
                assert!(
                    low <= world[axis] && world[axis] < low + width[axis],
                    "{at:?}: {world:?} is not in {cell:?}"
                );
                seen[axis][0] |= cell[axis] == 0;
                seen[axis][1] |= cell[axis] == cells[axis] - 1;
            }
        }
    }
    assert!(margins > 0, "the fit view's margin was clicked");
    assert_eq!(seen, [[true; 2]; 2], "both ends of both axes were clicked");

    // Either side of the seams the view is centred on: the last cells and
    // the first.
    let cell_at = |at| match stroke(Brush::Energy, &seam, logical, cells, at) {
        Some(Stroke::Energy { cell, .. }) => cell,
        made => panic!("{at:?} made {made:?}"),
    };
    let before = cell_at([399.5, 299.5]);
    let after = cell_at([400.5, 300.5]);
    assert_eq!([before[0], after[0]], [63, 0]);
    let mut rows = [before[1], after[1]];
    rows.sort_unstable();
    assert_eq!(rows, [0, 15]);

    assert_eq!(
        stroke(Brush::Energy, &fit, [0.0, 0.0], cells, [1.0, 1.0]),
        None
    );
    assert_eq!(
        stroke(Brush::Energy, &fit, logical, [0, 16], [400.0, 300.0]),
        None
    );
}

/// The removal brush is a disk about the world point under the click whose
/// radius is [`BRUSH_RADIUS_POINTS`] on screen: the world distance from the
/// click to the point that many points to its right, at any zoom. It is at
/// most half the shorter world side.
#[test]
fn a_removal_click_is_a_disk_of_fixed_screen_radius() {
    let extent = [32.0, 32.0];
    let logical = [800.0, 600.0];
    let fit = Viewport::fit(extent, [1600, 1200]).expect("fits");
    let mut radii = Vec::new();
    for zoom in [1.0, 4.0, 13.5] {
        let viewport = fit.with_view([12.0, 9.0], zoom).expect("views");
        let at = [380.0, 290.0];
        let Some(Stroke::Removal { center, radius }) =
            stroke(Brush::Removal, &viewport, logical, [32, 32], at)
        else {
            panic!("no removal at zoom {zoom}");
        };
        let world = viewport.logical_to_world(at, logical).expect("inside");
        assert_eq!(center, world);
        let right = viewport
            .logical_to_world([at[0] + BRUSH_RADIUS_POINTS, at[1]], logical)
            .expect("inside");
        assert!(right[0] > world[0], "no seam between them");
        let screen = right[0] - world[0];
        assert!(
            (radius - screen).abs() <= 1e-12 * screen,
            "zoom {zoom}: {radius} vs {screen}"
        );
        radii.push(radius);
    }
    assert!((radii[0] / radii[1] - 4.0).abs() < 1e-12, "{radii:?}");

    // A 16-point window shows 32 world units: 12 points is 24 units, held
    // to 16.
    let tiny = Viewport::fit(extent, [32, 32]).expect("fits");
    assert_eq!(
        stroke(Brush::Removal, &tiny, [16.0, 16.0], [32, 32], [8.0, 8.0]),
        Some(Stroke::Removal {
            center: tiny
                .logical_to_world([8.0, 8.0], [16.0, 16.0])
                .expect("inside"),
            radius: 16.0
        })
    );
    assert_eq!(
        stroke(Brush::Removal, &fit, logical, [32, 32], [10.0, 300.0]),
        None,
        "the margin"
    );
}

/// The payloads are the canonical interventions: an injection of the
/// stroke's energy, or a removal recycled as deaths are.
#[test]
fn a_stroke_is_its_canonical_intervention() {
    let energy = Stroke::Energy {
        cell: [3, 29],
        amount: BRUSH_ENERGY,
    };
    assert_eq!(energy.brush(), Brush::Energy);
    assert_eq!(
        energy.intervention(),
        Intervention::InjectEnergy {
            cell: [3, 29],
            amount: 64.0
        }
    );
    let removal = Stroke::Removal {
        center: [1.5, 30.25],
        radius: 2.5,
    };
    assert_eq!(removal.brush(), Brush::Removal);
    assert_eq!(
        removal.intervention(),
        Intervention::RemoveOrganisms {
            center: [1.5, 30.25],
            radius: 2.5,
            disposition: Disposition::Recycle,
        }
    );
}

/// Paused at tick 2, two strokes are accepted for the boundary of tick 2
/// and wait there; the next step applies both, and each reports what the
/// direct world stepped with both at tick 2 reports.
#[test]
fn a_paused_studio_session_applies_strokes_at_its_next_boundary() {
    let config = small();
    let mut direct = found(&config);
    step_with(&mut direct, &[]);
    step_with(&mut direct, &[]);
    let session = spawn(&config);
    session.send(Intent::Step).expect("offered");
    session.send(Intent::Step).expect("offered");
    let statuses = gather(&session, Vec::new(), |statuses| {
        statuses.contains(&Status::Stepped { tick: Tick::new(2) })
    });

    let target = direct.population().ids().nth(5).expect("an organism");
    let center = direct.population().get(target).expect("alive").position;
    let energy = Stroke::Energy {
        cell: [5, 9],
        amount: BRUSH_ENERGY,
    };
    let removal = Stroke::Removal {
        center,
        radius: 3.0,
    };
    session
        .send(Intent::Intervene {
            stroke: energy,
            seen: Tick::new(1),
        })
        .expect("offered");
    session
        .send(Intent::Intervene {
            stroke: removal,
            seen: Tick::new(2),
        })
        .expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        count(statuses, is_submitted) == 2
    });
    assert_eq!(
        submissions(&statuses),
        [
            (CommandId::new(1), energy, Tick::new(1), Tick::new(2)),
            (CommandId::new(2), removal, Tick::new(2), Tick::new(2)),
        ]
    );
    // Paused, the commands wait at their boundary.
    std::thread::sleep(OUTCOME_POLL * 10);
    let statuses = gather(&session, statuses, |_| true);
    assert_eq!(count(&statuses, is_finished), 0, "{statuses:?}");

    session.send(Intent::Step).expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        count(statuses, is_finished) == 2
    });
    assert!(statuses.contains(&Status::Stepped { tick: Tick::new(3) }));
    let reports = step_with(
        &mut direct,
        &[energy.intervention(), removal.intervention()],
    );
    let finished = applied(&statuses);
    assert_eq!(finished.len(), 2);
    for ((command, tick, published, energy), report) in finished.iter().zip(&reports) {
        assert_eq!(
            (*tick, *published),
            (Tick::new(2), Tick::new(3)),
            "{command:?}"
        );
        assert_eq!(energy.to_bits(), report.applied.to_bits(), "{command:?}");
    }
    assert!(finished[0].3 > 0.0 && finished[1].3 > 0.0, "{finished:?}");
    assert!(reports[1].removed >= 1);
    assert_eq!(direct.population().get(target), None, "removed");

    let closed = session.close().expect("closes");
    assert_eq!(closed.boundary.tick, Tick::new(3));
    assert_eq!(closed.world.digest(), direct.digest());
}

/// While the session runs, every stroke applies at the boundary it was
/// accepted for and is published by the next tick; replaying the applied
/// strokes at their ticks, in command order, on a direct world reproduces
/// every reported energy bit for bit and the session's final world.
#[test]
fn a_running_studio_session_applies_strokes_where_it_reports() {
    let config = small();
    let session = spawn(&config);
    session.send(Intent::Resume).expect("offered");
    let mut offered = 0;
    for index in 0..16_u32 {
        let stroke = if index % 2 == 0 {
            Stroke::Energy {
                cell: [(index * 7) % 32, (index * 11) % 32],
                amount: BRUSH_ENERGY,
            }
        } else {
            Stroke::Removal {
                center: [f64::from(index) * 1.9, 31.0 - f64::from(index) * 1.3],
                radius: 1.5,
            }
        };
        if session
            .send(Intent::Intervene {
                stroke,
                seen: Tick::new(0),
            })
            .is_ok()
        {
            offered += 1;
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    session.send(Intent::Pause).expect("offered");
    let statuses = gather(&session, Vec::new(), |statuses| {
        count(statuses, is_submitted) == offered
            && count(statuses, is_finished) == offered
            && statuses
                .iter()
                .any(|status| matches!(status, Status::Paused { .. }))
    });
    assert!(
        !statuses
            .iter()
            .any(|status| matches!(status, Status::Refused { .. })),
        "{statuses:?}"
    );
    let closed = session.close().expect("closes");

    let submitted = submissions(&statuses);
    let finished = applied(&statuses);
    assert_eq!(submitted.len(), finished.len());
    let mut ticks = Vec::new();
    for ((command, stroke, _, effective), (finished, tick, published, _)) in
        submitted.iter().zip(&finished)
    {
        assert_eq!(command, finished);
        assert_eq!(tick, effective, "{stroke:?}");
        assert_eq!(published.get(), tick.get() + 1);
        ticks.push(*tick);
    }
    ticks.dedup();
    assert!(ticks.len() > 1, "the strokes met more than one boundary");

    let mut direct = found(&config);
    while direct.tick() < closed.boundary.tick {
        let at = direct.tick();
        let here: Vec<_> = submitted
            .iter()
            .zip(&finished)
            .filter(|(_, (_, tick, _, _))| *tick == at)
            .collect();
        let interventions: Vec<_> = here
            .iter()
            .map(|((_, stroke, _, _), _)| stroke.intervention())
            .collect();
        let reports = step_with(&mut direct, &interventions);
        assert_eq!(reports.len(), here.len(), "tick {at:?}");
        for (report, (_, (command, _, _, energy))) in reports.iter().zip(&here) {
            assert_eq!(energy.to_bits(), report.applied.to_bits(), "{command:?}");
        }
    }
    assert_eq!(closed.world.digest(), direct.digest());
}

/// A stroke off the grid, or a disk wider than half the world, is refused
/// with the engine's reason and takes no identity; a stroke still waiting
/// at close never applies, and the close does not wait for it.
#[test]
fn a_studio_session_refuses_bad_strokes_and_closes_with_one_pending() {
    let config = small();
    let undisturbed = found(&config).digest();
    let session = spawn(&config);
    let off = Stroke::Energy {
        cell: [32, 0],
        amount: BRUSH_ENERGY,
    };
    let wide = Stroke::Removal {
        center: [4.0, 4.0],
        radius: 16.5,
    };
    let held = Stroke::Energy {
        cell: [31, 31],
        amount: BRUSH_ENERGY,
    };
    for stroke in [off, wide, held] {
        session
            .send(Intent::Intervene {
                stroke,
                seen: Tick::new(0),
            })
            .expect("offered");
    }
    let statuses = gather(&session, Vec::new(), |statuses| statuses.len() == 3);
    for (status, stroke) in statuses.iter().zip([off, wide]) {
        let Status::Refused {
            intent,
            kind,
            detail,
        } = status
        else {
            panic!("{status:?}");
        };
        assert_eq!(
            *intent,
            Intent::Intervene {
                stroke,
                seen: Tick::new(0)
            }
        );
        assert_eq!(*kind, "numerical-invalid", "{detail}");
    }
    assert_eq!(
        submissions(&statuses),
        [(CommandId::new(1), held, Tick::new(0), Tick::new(0))]
    );
    let started = Instant::now();
    let closed = session.close().expect("closes");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(closed.boundary.tick, Tick::new(0));
    assert_eq!(closed.world.digest(), undisturbed);
}
