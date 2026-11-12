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
use flife_manim::{Region, RegionEffect, RegionShape, Viewport};
use flife_runtime::Outcome;
use flife_studio::{
    BRUSH_ENERGY, BRUSH_INTRODUCTION, BRUSH_RADIUS_POINTS, Brush, BrushSettings, Intent,
    OUTCOME_POLL, Preview, SessionThread, Status, Stroke, Supplies, cell_region, drawing, drawn,
    material_for, preview, preview_region, stroke,
};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// No material, the preset's founder endowment, and the brushes' defaults.
const NONE: Supplies = Supplies {
    material: None,
    founder_energy: 2.0,
    settings: BrushSettings::DEFAULT,
};

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
                stroke(Brush::Energy, viewport, logical, cells, at, &NONE),
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
    let cell_at = |at| match stroke(Brush::Energy, &seam, logical, cells, at, &NONE) {
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
        stroke(Brush::Energy, &fit, [0.0, 0.0], cells, [1.0, 1.0], &NONE),
        None
    );
    assert_eq!(
        stroke(Brush::Energy, &fit, logical, [0, 16], [400.0, 300.0], &NONE),
        None
    );
}

/// A brush's preview at a pointer (plan §14.3) is the stroke a click there
/// makes, for every brush, at fit, zoomed and across the seams: for a cell,
/// with that cell's world rectangle, which holds the world point under the
/// pointer. Without a pointer or a content area, and in the letterbox
/// margin, it is off the world; the material brush over the world with no
/// material says so.
#[test]
fn a_preview_is_the_stroke_a_click_there_makes() {
    let extent = [32.0, 32.0];
    let cells = [64, 16];
    let logical = [800.0, 600.0];
    let fit = Viewport::fit(extent, [1600, 1200]).expect("fits");
    let zoomed = fit.with_view([3.3, 30.1], 2.75).expect("views");
    let seam = fit.with_view([0.0, 0.0], 2.0).expect("views");
    let material = material_for(&[0.5; flife_engine::MATERIAL_CHANNELS]);
    let supplied = Supplies { material, ..NONE };
    let brushes = [
        Brush::Energy,
        Brush::Removal,
        Brush::Material,
        Brush::Introduction,
    ];
    let (mut strokes, mut off, mut unsupplied) = (0, 0, 0);
    for viewport in [fit, zoomed, seam] {
        for (at, world, _) in lattice(&viewport, logical, cells) {
            for brush in brushes {
                for supplies in [supplied, NONE] {
                    let shown =
                        preview(brush, &viewport, Some(logical), cells, Some(at), &supplies);
                    match stroke(brush, &viewport, logical, cells, at, &supplies) {
                        Some(made) => {
                            let region = made.cell().map(|cell| cell_region(extent, cells, cell));
                            assert_eq!(
                                shown,
                                Preview::Stroke {
                                    stroke: made,
                                    region
                                }
                            );
                            if let (Some([[x0, x1], [y0, y1]]), Some([x, y])) = (region, world) {
                                assert!(
                                    x0 <= x && x < x1 && y0 <= y && y < y1,
                                    "{at:?}: {world:?} outside {region:?}"
                                );
                            }
                            // Outlined: the cell, or the removal's disc,
                            // and whether the stroke removes energy.
                            let shape = match (made, region) {
                                (_, Some(bounds)) => RegionShape::Rectangle(bounds),
                                (Stroke::Removal { center, radius, .. }, None) => {
                                    RegionShape::Disc { center, radius }
                                }
                                _ => panic!("{made:?} has neither a cell nor a disc"),
                            };
                            assert_eq!(
                                preview_region(&shown),
                                Some(Region {
                                    shape,
                                    effect: if brush == Brush::Removal {
                                        RegionEffect::Removes
                                    } else {
                                        RegionEffect::Adds
                                    }
                                })
                            );
                            strokes += 1;
                        }
                        None if world.is_some() => {
                            assert_eq!((brush, supplies.material), (Brush::Material, None));
                            assert_eq!(shown, Preview::NoMaterial, "{at:?}");
                            assert_eq!(preview_region(&shown), None);
                            unsupplied += 1;
                        }
                        None => {
                            assert_eq!(shown, Preview::OffWorld, "{brush:?} at {at:?}");
                            assert_eq!(preview_region(&shown), None);
                            off += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(
        strokes > 0 && off > 0 && unsupplied > 0,
        "{strokes} {off} {unsupplied}"
    );
    for brush in brushes {
        let at = Some([400.0, 300.0]);
        assert_eq!(
            preview(brush, &fit, Some(logical), cells, None, &supplied),
            Preview::OffWorld
        );
        assert_eq!(
            preview(brush, &fit, None, cells, at, &supplied),
            Preview::OffWorld
        );
    }
    assert_eq!(
        cell_region(extent, cells, [63, 15]),
        [[31.5, 32.0], [30.0, 32.0]]
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
        let Some(Stroke::Removal {
            center,
            radius,
            disposition: Disposition::Recycle,
        }) = stroke(Brush::Removal, &viewport, logical, [32, 32], at, &NONE)
        else {
            panic!("no recycled removal at zoom {zoom}");
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
        stroke(
            Brush::Removal,
            &tiny,
            [16.0, 16.0],
            [32, 32],
            [8.0, 8.0],
            &NONE
        ),
        Some(Stroke::Removal {
            center: tiny
                .logical_to_world([8.0, 8.0], [16.0, 16.0])
                .expect("inside"),
            radius: 16.0,
            disposition: Disposition::Recycle,
        })
    );
    assert_eq!(
        stroke(
            Brush::Removal,
            &fit,
            logical,
            [32, 32],
            [10.0, 300.0],
            &NONE
        ),
        None,
        "the margin"
    );
}

/// A drag of the removal brush draws a disk about the world point under its
/// press whose radius is the drag's length on screen, in any direction:
/// the world distance from the press to the point that far to its right,
/// at any zoom, and for a drag of [`BRUSH_RADIUS_POINTS`] exactly a
/// click's disk there. It is at most half the shorter world side, and the
/// release may be anywhere; a press in the margin, a drag of no length and
/// a nonfinite one draw nothing. While drawn, its preview is that disk,
/// outlined as a click's is.
#[test]
fn a_removal_drag_draws_a_disk_of_its_length() {
    let extent = [32.0, 32.0];
    let logical = [800.0, 600.0];
    let fit = Viewport::fit(extent, [1600, 1200]).expect("fits");
    let at = [380.0, 290.0];
    for zoom in [1.0, 4.0, 13.5] {
        let viewport = fit.with_view([12.0, 9.0], zoom).expect("views");
        let world = viewport.logical_to_world(at, logical).expect("inside");
        for (length, [dx, dy]) in [(30.0, [0.6, 0.8]), (57.5, [-1.0, 0.0]), (5.0, [0.0, -1.0])] {
            let edge = [at[0] + length * dx, at[1] + length * dy];
            let Some(Stroke::Removal {
                center,
                radius,
                disposition: Disposition::Recycle,
            }) = drawn(&viewport, logical, at, edge)
            else {
                panic!("no recycled disk at zoom {zoom}, length {length}");
            };
            assert_eq!(center, world);
            let right = viewport
                .logical_to_world([at[0] + length, at[1]], logical)
                .expect("inside");
            assert!(right[0] > world[0], "no seam between them");
            let screen = right[0] - world[0];
            assert!(
                (radius - screen).abs() <= 1e-12 * screen,
                "zoom {zoom}, length {length}: {radius} vs {screen}"
            );
            // Its preview while drawn, with each disposition the release
            // would apply, and the region outlined: the same disc.
            for disposition in Disposition::ALL {
                let shown = drawing(&viewport, Some(logical), at, Some(edge), disposition);
                assert_eq!(
                    shown,
                    Preview::Drawn {
                        stroke: Some(Stroke::Removal {
                            center,
                            radius,
                            disposition
                        })
                    }
                );
                assert_eq!(
                    preview_region(&shown),
                    Some(Region {
                        shape: RegionShape::Disc { center, radius },
                        effect: RegionEffect::Removes
                    })
                );
            }
        }
        assert_eq!(
            drawn(&viewport, logical, at, [at[0], at[1] + BRUSH_RADIUS_POINTS]),
            stroke(Brush::Removal, &viewport, logical, [32, 32], at, &NONE),
            "zoom {zoom}"
        );
    }

    // A 16-point window shows 32 world units: a 12-point drag is 24 units,
    // held to 16, released off the window.
    let tiny = Viewport::fit(extent, [32, 32]).expect("fits");
    assert_eq!(
        drawn(&tiny, [16.0, 16.0], [8.0, 8.0], [8.0, 20.0]),
        Some(Stroke::Removal {
            center: tiny
                .logical_to_world([8.0, 8.0], [16.0, 16.0])
                .expect("inside"),
            radius: 16.0,
            disposition: Disposition::Recycle,
        })
    );
    let margin = [10.0, 300.0];
    assert_eq!(
        drawn(&fit, logical, margin, [60.0, 300.0]),
        None,
        "the margin"
    );
    assert_eq!(drawn(&fit, logical, at, at), None, "no length");
    for edge in [[f64::NAN, at[1]], [f64::INFINITY, at[1]]] {
        assert_eq!(drawn(&fit, logical, at, edge), None, "{edge:?}");
    }
    // Pressed in the margin, or with no content area or pointer, the
    // preview names no disk and outlines nothing.
    for shown in [
        drawing(
            &fit,
            Some(logical),
            margin,
            Some([60.0, 300.0]),
            Disposition::Recycle,
        ),
        drawing(&fit, None, at, Some([400.0, 300.0]), Disposition::Remove),
        drawing(&fit, Some(logical), at, None, Disposition::Dissipate),
    ] {
        assert_eq!(shown, Preview::Drawn { stroke: None });
        assert_eq!(preview_region(&shown), None);
    }
}

/// The material and introduction brushes name the cell the energy brush
/// names under every click. The material brush carries the supplied
/// composition and makes nothing without one; the introduction brush
/// carries [`BRUSH_INTRODUCTION`] organisms at the supplied endowment.
#[test]
fn material_and_introduction_clicks_carry_their_supplies() {
    let logical = [800.0, 600.0];
    let cells = [64, 16];
    let viewport = Viewport::fit([32.0, 32.0], [1600, 1200])
        .expect("fits")
        .with_view([5.0, 27.0], 1.7)
        .expect("views");
    let mut direction = [0.0; 32];
    direction[3] = 0.6;
    direction[17] = -0.8;
    let supplied = Supplies {
        material: Some(direction),
        founder_energy: 2.5,
        settings: BrushSettings::DEFAULT,
    };
    let mut made = 0;
    for (at, _, energy) in lattice(&viewport, logical, cells) {
        let material = stroke(Brush::Material, &viewport, logical, cells, at, &supplied);
        let introduction = stroke(
            Brush::Introduction,
            &viewport,
            logical,
            cells,
            at,
            &supplied,
        );
        assert_eq!(
            stroke(Brush::Material, &viewport, logical, cells, at, &NONE),
            None,
            "{at:?}"
        );
        let Some(Stroke::Energy { cell, .. }) = energy else {
            assert_eq!((material, introduction), (None, None), "{at:?}");
            continue;
        };
        assert_eq!(
            material,
            Some(Stroke::Material {
                cell,
                amount: BRUSH_ENERGY,
                direction
            })
        );
        assert_eq!(
            introduction,
            Some(Stroke::Introduction {
                cell,
                count: BRUSH_INTRODUCTION,
                energy: 2.5
            })
        );
        made += 1;
    }
    assert!(made > 100, "{made}");
}

/// An organism's material is its affinity's unit direction: within the
/// unit ball as the engine checks it (`‖·‖₂² ≤ 1`, summed in channel
/// order), parallel to the affinity, and so the composition of unit norm
/// with the largest `a·z`. A zero or nonfinite affinity has none.
#[test]
fn an_organisms_material_is_its_affinity_direction() {
    let config = small();
    let world = found(&config);
    let mut checked = 0;
    for id in world.population().ids().take(24) {
        let snapshot = world
            .organism_snapshot(id)
            .expect("inspects")
            .expect("alive");
        let affinity = *snapshot.traits.affinity();
        let norm = affinity.iter().map(|a| a * a).sum::<f64>().sqrt();
        let Some(material) = material_for(&affinity) else {
            assert_eq!(norm, 0.0, "{id:?}");
            continue;
        };
        let squared = material.iter().fold(0.0, |total, &z| total + z * z);
        assert!(squared <= 1.0 && squared > 1.0 - 1e-14, "{squared}");
        for (z, a) in material.iter().zip(&affinity) {
            assert!((z * norm - a).abs() <= 1e-14 * norm, "{z} vs {a}/{norm}");
        }
        let along: f64 = material.iter().zip(&affinity).map(|(z, a)| z * a).sum();
        assert!(
            (along - norm).abs() <= 1e-14 * norm.max(1.0),
            "{along} vs {norm}"
        );
        checked += 1;
    }
    assert!(checked > 0);
    assert_eq!(material_for(&[0.0; 32]), None);
    let mut broken = [0.1; 32];
    broken[9] = f64::NAN;
    assert_eq!(material_for(&broken), None);
}

/// FL-022's amount, count and disposition: under every click, each
/// setting the keys give a brush makes the stroke the defaults make, at
/// that setting's amount, count or disposition, and nothing else changes:
/// the cell, the disk, the material and the endowment are the click's.
/// The amounts are exact powers of two of the defaults: 1 to 4,096 energy
/// and 1 to 64 organisms.
#[test]
fn each_setting_scales_its_brush_and_sends_the_removal() {
    let logical = [800.0, 600.0];
    let cells = [64, 16];
    let viewport = Viewport::fit([32.0, 32.0], [1600, 1200])
        .expect("fits")
        .with_view([5.0, 27.0], 1.7)
        .expect("views");
    let mut direction = [0.0; 32];
    direction[3] = 0.6;
    direction[17] = -0.8;
    let defaults = Supplies {
        material: Some(direction),
        founder_energy: 2.5,
        settings: BrushSettings::DEFAULT,
    };
    // Every setting of each brush, from the lowest up.
    let every = |brush: Brush| {
        let mut lowest = BrushSettings::DEFAULT;
        while lowest.scaled(brush, false) != lowest {
            lowest = lowest.scaled(brush, false);
        }
        let mut all = vec![lowest];
        while all[all.len() - 1].scaled(brush, true) != all[all.len() - 1] {
            all.push(all[all.len() - 1].scaled(brush, true));
        }
        all
    };
    let amounts: Vec<f64> = (-6..=6).map(|step| 64.0 * 2.0_f64.powi(step)).collect();
    for brush in [Brush::Energy, Brush::Material] {
        let seen: Vec<_> = every(brush)
            .iter()
            .map(|settings| settings.amount(brush).expect("an amount"))
            .collect();
        assert_eq!(seen, amounts, "{brush:?}");
        assert_eq!((seen[0], seen[12]), (1.0, 4_096.0));
    }
    let counts: Vec<u32> = every(Brush::Introduction)
        .iter()
        .map(|settings| settings.count())
        .collect();
    assert_eq!(counts, [1, 2, 4, 8, 16, 32, 64]);
    assert_eq!(every(Brush::Removal), [BrushSettings::DEFAULT]);
    let dispositions = [
        BrushSettings::DEFAULT,
        BrushSettings::DEFAULT.next_disposition(),
        BrushSettings::DEFAULT.next_disposition().next_disposition(),
    ];
    assert_eq!(
        dispositions.map(BrushSettings::disposition),
        [
            Disposition::Recycle,
            Disposition::Dissipate,
            Disposition::Remove
        ]
    );
    assert_eq!(
        dispositions[2].next_disposition(),
        BrushSettings::DEFAULT,
        "round again"
    );
    // A brush's settings move only its own amount.
    let doubled = BrushSettings::DEFAULT.scaled(Brush::Energy, true);
    assert_eq!(doubled.amount(Brush::Material), Some(BRUSH_ENERGY));
    assert_eq!(doubled.count(), BRUSH_INTRODUCTION);
    assert_eq!(doubled.disposition(), Disposition::Recycle);

    let settings = [Brush::Energy, Brush::Material, Brush::Introduction].map(every);
    let mut made = 0;
    // Every seventh click of the lattice: thousands of cells, both seams.
    for (at, _, energy) in lattice(&viewport, logical, cells).into_iter().step_by(7) {
        let Some(Stroke::Energy { cell, .. }) = energy else {
            continue;
        };
        let click = |brush, settings| {
            stroke(
                brush,
                &viewport,
                logical,
                cells,
                at,
                &Supplies {
                    settings,
                    ..defaults
                },
            )
        };
        for &set in &settings[0] {
            let amount = set.amount(Brush::Energy).expect("an amount");
            assert_eq!(
                click(Brush::Energy, set),
                Some(Stroke::Energy { cell, amount })
            );
        }
        for &set in &settings[1] {
            let amount = set.amount(Brush::Material).expect("an amount");
            assert_eq!(
                click(Brush::Material, set),
                Some(Stroke::Material {
                    cell,
                    amount,
                    direction
                })
            );
        }
        for &set in &settings[2] {
            assert_eq!(
                click(Brush::Introduction, set),
                Some(Stroke::Introduction {
                    cell,
                    count: set.count(),
                    energy: 2.5
                })
            );
        }
        let Some(Stroke::Removal { center, radius, .. }) =
            click(Brush::Removal, BrushSettings::DEFAULT)
        else {
            panic!("no removal at {at:?}");
        };
        for set in dispositions {
            let removal = click(Brush::Removal, set).expect("a removal");
            assert_eq!(
                removal,
                Stroke::Removal {
                    center,
                    radius,
                    disposition: set.disposition()
                }
            );
            assert_eq!(
                removal.intervention(),
                Intervention::RemoveOrganisms {
                    center,
                    radius,
                    disposition: set.disposition()
                }
            );
        }
        made += 1;
    }
    assert!(made > 100, "{made}");
}

/// The payloads are the canonical interventions: an injection of the
/// stroke's energy, or a removal with its disposition, recycled as deaths
/// are by default.
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
        disposition: Disposition::Recycle,
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
    // `disposed` sends a removal elsewhere, and leaves any other stroke.
    for disposition in Disposition::ALL {
        assert_eq!(
            removal.disposed(disposition).intervention(),
            Intervention::RemoveOrganisms {
                center: [1.5, 30.25],
                radius: 2.5,
                disposition,
            }
        );
        assert_eq!(energy.disposed(disposition), energy);
    }
    let mut direction = [0.0; 32];
    direction[0] = -1.0;
    let material = Stroke::Material {
        cell: [7, 8],
        amount: 12.5,
        direction,
    };
    assert_eq!(material.brush(), Brush::Material);
    assert_eq!(
        material.intervention(),
        Intervention::InjectMaterial {
            cell: [7, 8],
            amount: 12.5,
            direction,
        }
    );
    let introduction = Stroke::Introduction {
        cell: [30, 1],
        count: 8,
        energy: 2.0,
    };
    assert_eq!(introduction.brush(), Brush::Introduction);
    assert_eq!(
        introduction.intervention(),
        Intervention::IntroduceOrganisms {
            cell: [30, 1],
            count: 8,
            energy: 2.0,
        }
    );
    assert_eq!(
        [
            Brush::Energy,
            Brush::Removal,
            Brush::Material,
            Brush::Introduction
        ]
        .map(Brush::name),
        ["energy", "removal", "material", "introduction"]
    );
}

/// Paused at tick 3, a material stroke of a living organism's affinity
/// direction and an introduction apply at the next step with what the
/// direct world stepped with both reports: the energy each moved, bit for
/// bit, and eight introduced organisms; the session ends on that world.
#[test]
fn a_paused_studio_session_applies_material_and_introduction_strokes() {
    let config = small();
    let mut direct = found(&config);
    for _ in 0..3 {
        step_with(&mut direct, &[]);
    }
    let session = spawn(&config);
    for _ in 0..3 {
        session.send(Intent::Step).expect("offered");
    }
    let statuses = gather(&session, Vec::new(), |statuses| {
        statuses.contains(&Status::Stepped { tick: Tick::new(3) })
    });
    let organism = direct.population().ids().nth(11).expect("an organism");
    let affinity = *direct
        .organism_snapshot(organism)
        .expect("inspects")
        .expect("alive")
        .traits
        .affinity();
    let material = Stroke::Material {
        cell: [20, 6],
        amount: BRUSH_ENERGY,
        direction: material_for(&affinity).expect("a direction"),
    };
    let introduction = Stroke::Introduction {
        cell: [2, 29],
        count: BRUSH_INTRODUCTION,
        energy: config.config().population.initial_energy,
    };
    for stroke in [material, introduction] {
        session
            .send(Intent::Intervene {
                stroke,
                seen: Tick::new(3),
            })
            .expect("offered");
    }
    let statuses = gather(&session, statuses, |statuses| {
        count(statuses, is_submitted) == 2
    });
    assert_eq!(
        submissions(&statuses),
        [
            (CommandId::new(1), material, Tick::new(3), Tick::new(3)),
            (CommandId::new(2), introduction, Tick::new(3), Tick::new(3)),
        ]
    );
    session.send(Intent::Step).expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        count(statuses, is_finished) == 2
    });
    let reports = match direct
        .step_with(&[material.intervention(), introduction.intervention()])
        .expect("steps")
    {
        Stepped::Published(report) => {
            assert_eq!(report.introductions.len(), 8);
            report.interventions
        }
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    };
    let finished = applied(&statuses);
    assert_eq!(finished.len(), 2);
    for ((command, tick, published, energy), report) in finished.iter().zip(&reports) {
        assert_eq!(
            (*tick, *published),
            (Tick::new(3), Tick::new(4)),
            "{command:?}"
        );
        assert_eq!(energy.to_bits(), report.applied.to_bits(), "{command:?}");
    }
    assert_eq!(reports[1].introduced, 8);
    assert_eq!(
        finished[1].3,
        8.0 * config.config().population.initial_energy
    );
    let closed = session.close().expect("closes");
    assert_eq!(closed.boundary.tick, Tick::new(4));
    assert_eq!(closed.world.digest(), direct.digest());
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
        disposition: Disposition::Recycle,
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
/// every reported energy bit for bit and the session's final world. The
/// strokes take amounts the keys set and each removal disposition.
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
                amount: BRUSH_ENERGY * f64::from(1_u32 << (index % 6)) / 4.0,
            }
        } else {
            Stroke::Removal {
                center: [f64::from(index) * 1.9, 31.0 - f64::from(index) * 1.3],
                radius: 1.5,
                disposition: Disposition::ALL[(index as usize / 2) % 3],
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
        disposition: Disposition::Remove,
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
