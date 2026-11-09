//! FL-017 evidence (plan §9.5, §13.2, §13.5, Appendix B.3, C.7, C.15):
//! render epochs are exactly one committed world, in stable identity order,
//! independent of storage arrangement, preallocated and bounded.
//!
//! Every view is compared with the world's own rows. The field texture is
//! compared with an independent block-mean oracle.

mod common;

use common::{ecology, found, step};
use flife_core::CoreError;
use flife_core::ids::OrganismId;
use flife_engine::{Arrangement, World};
use flife_render::{
    Bounds, Dtype, Epoch, FIELD_TEXELS_MAX, FieldChannel, FieldTexture, Gap, Lease, Match,
    Publication, Publisher, ROUNDING_MARGIN, View, pairs,
};

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn publisher(world: &World) -> Publisher {
    Publisher::new(world, 3, || {}).expect("a publisher")
}

/// Offer `world` and lease what the feed then shows.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn offer_and_lease(publisher: &mut Publisher, world: &World) -> Lease {
    let publication = publisher.offer(world).expect("an offer");
    assert!(
        matches!(publication, Publication::Published { .. }),
        "{publication:?}"
    );
    publisher
        .feed()
        .viewer(1)
        .expect("a viewer")
        .latest()
        .expect("a lease")
}

/// The F32 coordinate an epoch must show for `coordinate` on a `length`
/// axis, computed independently of the crate.
fn shown(coordinate: f64, length: f64) -> f32 {
    let rounded = coordinate as f32;
    if f64::from(rounded) >= f64::from(length as f32) {
        0.0
    } else {
        rounded
    }
}

/// Every view of `epoch` against `world`'s committed rows.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn assert_shows(epoch: &Epoch, world: &World) {
    let population = world.population();
    let rows: Vec<_> = population.iter().collect();
    let grid = world.field().grid();
    let [width, height] = [grid.world_width(), grid.world_height()];
    assert_eq!(epoch.tick(), world.tick());
    assert_eq!(epoch.resource(), world.epoch());
    assert_eq!(epoch.step(), world.step_size());
    assert_eq!(epoch.maximum_speed(), world.maximum_speed());
    assert_eq!(epoch.extent(), [width, height]);
    assert_eq!(epoch.capacity(), population.capacity());
    assert_eq!(epoch.len(), rows.len());
    for (row, organism) in rows.iter().enumerate() {
        assert_eq!(epoch.ids()[row], organism.id.get());
        assert_eq!(epoch.id_at(row), Some(organism.id));
        assert_eq!(epoch.row(organism.id), Some(row));
        assert_eq!(
            epoch.positions()[row],
            [
                shown(organism.position[0], width),
                shown(organism.position[1], height)
            ]
        );
        assert_eq!(
            epoch.velocities()[row],
            [organism.velocity[0] as f32, organism.velocity[1] as f32]
        );
        assert_eq!(epoch.energies()[row], organism.energy as f32);
        assert_eq!(epoch.lineages()[row], organism.lineage.get());
        assert_eq!(epoch.generations()[row], organism.generation.get());
        let topology = population.genome(organism.id).expect("genome").topology();
        assert_eq!(usize::from(epoch.topologies()[row]), topology.index());
    }
    let metrics = epoch.metrics();
    assert_eq!(metrics.tick, world.tick());
    assert_eq!(metrics.live, rows.len() as u64);
    let energy: f64 = rows.iter().map(|organism| organism.energy).sum();
    assert_eq!(metrics.organism_energy.to_bits(), energy.to_bits());
    assert_eq!(
        metrics.field_energy.to_bits(),
        world.field().energy_total().to_bits()
    );
    assert_eq!(
        metrics.max_generation,
        rows.iter().map(|o| o.generation.get()).max().unwrap_or(0)
    );
    assert_eq!(metrics.events, world.events());
    epoch.check().expect("every declared bound holds");
}

/// The block-mean oracle: each texel's mean energy density, summed in F64
/// over its cells in row-major order.
fn oracle(cells: [u32; 2], cell_size: [f64; 2], plane: &[f32]) -> (Vec<f32>, [u32; 2]) {
    let block = cells.map(|n| n.div_ceil(FIELD_TEXELS_MAX));
    let texels = [cells[0].div_ceil(block[0]), cells[1].div_ceil(block[1])];
    let mut values = Vec::new();
    for ty in 0..texels[1] {
        for tx in 0..texels[0] {
            let (mut sum, mut n) = (0.0_f64, 0_u32);
            for y in ty * block[1]..((ty + 1) * block[1]).min(cells[1]) {
                for x in tx * block[0]..((tx + 1) * block[0]).min(cells[0]) {
                    sum += f64::from(plane[(y * cells[0] + x) as usize]);
                    n += 1;
                }
            }
            values.push((sum / (f64::from(n) * cell_size[0] * cell_size[1])) as f32);
        }
    }
    (values, texels)
}

/// B.3: an epoch is exactly one committed world. At the founding tick and
/// after births and deaths, every view, the metrics and the field texture
/// match the world's own committed state, and every declared layout holds.
#[test]
fn an_epoch_shows_exactly_one_committed_world() {
    let config = ecology(&[]);
    let mut world = found(&config, Arrangement::FreeList);
    let mut publisher = publisher(&world);
    let founding = publisher
        .feed()
        .viewer(1)
        .expect("viewer")
        .latest()
        .expect("lease");
    assert_eq!(founding.id().get(), 1);
    assert_eq!(founding.prior(), None);
    assert_shows(&founding, &world);
    drop(founding);

    let (mut births, mut deaths) = (0, 0);
    let mut previous = publisher.latest();
    for _ in 0..288 {
        let report = step(&mut world);
        births += report.births.len();
        deaths += report.deaths.len();
        let lease = offer_and_lease(&mut publisher, &world);
        assert_eq!(lease.prior(), Some(previous));
        assert_eq!(lease.id().get(), previous.id.get() + 1);
        assert_shows(&lease, &world);
        previous = publisher.latest();
    }
    assert!(births > 0 && deaths > 0, "{births} births, {deaths} deaths");

    let lease = publisher
        .feed()
        .viewer(1)
        .expect("viewer")
        .latest()
        .expect("lease");
    let layouts = lease.layouts();
    let declared: Vec<_> = layouts
        .iter()
        .map(|layout| (layout.view, layout.dtype, layout.components))
        .collect();
    assert_eq!(
        declared,
        vec![
            (View::Identity, Dtype::U64, 1),
            (View::Position, Dtype::F32, 2),
            (View::Velocity, Dtype::F32, 2),
            (View::Energy, Dtype::F32, 1),
            (View::Lineage, Dtype::U64, 1),
            (View::Generation, Dtype::U32, 1),
            (View::Topology, Dtype::U8, 1),
            (View::Field, Dtype::F32, 1),
        ]
    );
    assert_eq!(
        layouts.map(|layout| layout.view).to_vec(),
        View::ALL.to_vec()
    );
    for layout in &layouts[..7] {
        assert_eq!(layout.count, lease.len());
    }
    assert_eq!(layouts[1].bounds, Bounds::Wrapped([32.0, 32.0]));
    assert_eq!(layouts[2].bounds, Bounds::Magnitude(4.0));
    assert_eq!(layouts[7].count, 32 * 32);

    let texture = lease.field();
    let grid = world.field().grid();
    assert_eq!(texture.channel(), FieldChannel::EnergyDensity);
    assert_eq!(texture.cells(), [32, 32]);
    assert_eq!(texture.block(), [1, 1]);
    assert_eq!(texture.texels(), [32, 32]);
    assert_eq!(
        texture.texel_size(),
        [grid.cell_width(), grid.cell_height()]
    );
    let (expected, _) = oracle(
        [32, 32],
        [grid.cell_width(), grid.cell_height()],
        world.field().energy(),
    );
    assert_eq!(texture.values(), expected.as_slice());
}

/// §9.5: a large grid's texture is bounded by averaging whole-cell blocks;
/// an axis that does not divide evenly ends in a partial block. Planes of
/// the wrong size and empty grids are refused.
#[test]
fn a_large_field_is_a_bounded_block_mean_texture() {
    let cells = [1030_u32, 515];
    let cell_size = [0.5, 0.25];
    let plane: Vec<f32> = (0..cells[0] * cells[1])
        .map(|i| ((i * 7919) % 1013) as f32 * 0.125)
        .collect();
    let texture = FieldTexture::from_plane(cells, cell_size, &plane).expect("texture");
    let (expected, texels) = oracle(cells, cell_size, &plane);
    assert_eq!(texture.block(), [3, 2]);
    assert_eq!(texels, [344, 258]);
    assert_eq!(texture.texels(), texels);
    assert!(texels.iter().all(|&n| n <= FIELD_TEXELS_MAX));
    assert_eq!(texture.texel_size(), [1.5, 0.5]);
    assert_eq!(texture.values(), expected.as_slice());
    assert_eq!(texture.layout().count, 344 * 258);

    let exact = FieldTexture::from_plane([512, 1], [1.0, 1.0], &[2.0; 512]).expect("texture");
    assert_eq!((exact.block(), exact.texels()), ([1, 1], [512, 1]));
    let halved = FieldTexture::from_plane([1024, 1], [1.0, 1.0], &[2.0; 1024]).expect("texture");
    assert_eq!((halved.block(), halved.texels()), ([2, 1], [512, 1]));
    assert!(halved.values().iter().all(|&v| v == 2.0));

    assert!(matches!(
        FieldTexture::from_plane(cells, cell_size, &plane[1..]),
        Err(CoreError::LayoutMismatch { .. })
    ));
    assert!(matches!(
        FieldTexture::from_plane([0, 4], cell_size, &[]),
        Err(CoreError::LayoutMismatch { .. })
    ));
}

/// C.7: two storage arrangements of one stable-identity world occupy
/// different slots yet publish identical epochs at every tick, and the
/// identity matching between consecutive epochs is identical too.
#[test]
fn c7_two_storage_arrangements_publish_identical_epochs() {
    let config = ecology(&[]);
    let mut compact = found(&config, Arrangement::Compact);
    let mut free_list = found(&config, Arrangement::FreeList);
    let mut a = publisher(&compact);
    let mut b = publisher(&free_list);
    let mut previous: Option<(Lease, Lease)> = None;
    let mut layouts_differed = 0;
    for _ in 0..288 {
        step(&mut compact);
        step(&mut free_list);
        assert_eq!(compact.digest(), free_list.digest());
        if compact.population().slots() != free_list.population().slots() {
            layouts_differed += 1;
        }
        let x = offer_and_lease(&mut a, &compact);
        let y = offer_and_lease(&mut b, &free_list);
        assert_eq!(*x, *y, "tick {}", compact.tick().get());
        if let Some((px, py)) = &previous {
            let mx: Vec<Match> = pairs(px, &x).collect();
            let my: Vec<Match> = pairs(py, &y).collect();
            assert_eq!(mx, my);
        }
        previous = Some((x, y));
    }
    assert!(
        layouts_differed > 0,
        "the two arrangements never placed organisms differently"
    );
}

/// §13.5: matching is by identity. Between consecutive epochs, continuing
/// organisms pair with themselves and every birth and death of the tick
/// appears once. When the free list gives a dead organism's slot to a
/// newborn, possibly ticks later, the newborn is a birth and the dead
/// occupant is gone: the slot never links two identities.
#[test]
fn a_reused_slot_never_pairs_two_identities() {
    let config = ecology(&[]);
    let mut world = found(&config, Arrangement::FreeList);
    let mut publisher = publisher(&world);
    let mut prior = publisher
        .feed()
        .viewer(2)
        .expect("viewer")
        .latest()
        .expect("lease");
    let mut last: Vec<Option<OrganismId>> = world.population().slots().to_vec();
    let (mut reused, mut total_births, mut total_deaths, mut births_after_death) = (0, 0, 0, 0);
    for _ in 0..1024 {
        let report = step(&mut world);
        if total_deaths > 0 {
            births_after_death += report.births.len();
        }
        total_births += report.births.len();
        total_deaths += report.deaths.len();
        let current = offer_and_lease(&mut publisher, &world);
        let (mut born, mut died, mut continuing) = (Vec::new(), Vec::new(), 0);
        for found in pairs(&prior, &current) {
            match found {
                Match::Continuing {
                    prior: p,
                    current: c,
                } => {
                    assert_eq!(prior.ids()[p], current.ids()[c]);
                    continuing += 1;
                }
                Match::Born { current: c } => born.push(current.ids()[c]),
                Match::Died { prior: p } => died.push(prior.ids()[p]),
            }
        }
        let births: Vec<u64> = report.births.iter().map(|b| b.child.get()).collect();
        let deaths: Vec<u64> = report.deaths.iter().map(|d| d.id.get()).collect();
        assert_eq!(born, births);
        assert_eq!(died, deaths);
        assert_eq!(continuing + born.len(), current.len());
        for (slot, occupant) in world.population().slots().iter().enumerate() {
            let Some(after) = *occupant else { continue };
            if let Some(Some(before)) = last.get(slot).copied()
                && before != after
            {
                reused += 1;
                assert!(
                    births.contains(&after.get()),
                    "{after} entered a reused slot unborn"
                );
                assert_eq!(current.row(before), None, "{before} still shown");
                assert_eq!(prior.row(after), None);
            }
            if let Some(entry) = last.get_mut(slot) {
                *entry = Some(after);
            } else {
                last.push(Some(after));
            }
        }
        prior = current;
    }
    assert!(
        reused > 0,
        "no slot was reused: {total_births} births, {total_deaths} deaths, \
         {births_after_death} births after the first death, {} live",
        world.population().len()
    );
}

/// §9.5: every slot is allocated once, at the population ceiling and the
/// texture size, and no projection reallocates, up to the ceiling itself.
#[test]
fn slots_are_allocated_once_and_never_grow() {
    let config = ecology(&[("population", "maximum_count", "72")]);
    let mut world = found(&config, Arrangement::FreeList);
    let mut publisher = publisher(&world);
    let row = 8 + 8 + 8 + 4 + 8 + 4 + 1;
    let slot = 72 * row + 32 * 32 * 4;
    assert_eq!(publisher.reserved_bytes(), 3 * slot);
    let viewer = publisher.feed().viewer(1).expect("viewer");
    let (mut peak, mut ceiling) = (0, false);
    for _ in 0..512 {
        match world.step().expect("advance") {
            flife_engine::Stepped::Published(_) => {}
            flife_engine::Stepped::Paused(_) => {
                ceiling = true;
                break;
            }
        }
        publisher.offer(&world).expect("offer");
        let lease = viewer.latest().expect("lease");
        assert_eq!(lease.reserved_bytes(), slot);
        peak = peak.max(lease.len());
        assert_eq!(publisher.reserved_bytes(), 3 * slot);
    }
    assert!(ceiling, "the population never reached its ceiling");
    assert!(peak > 60, "the population only reached {peak}");
}

/// C.15: latest-value publication can skip many ticks. A gap's travel
/// bound is `ticks · Δt · maximum speed`; a one-tick gap allows
/// minimum-image interpolation, and a gap long enough to cross half the
/// world does not. Epochs out of order or from different worlds are
/// refused.
#[test]
fn c15_a_long_skipped_gap_is_ambiguous_and_a_short_one_is_not() {
    let config = ecology(&[]);
    let mut world = found(&config, Arrangement::FreeList);
    let mut publisher = publisher(&world);
    let feed = publisher.feed();
    let first = feed.viewer(2).expect("viewer").latest().expect("lease");
    step(&mut world);
    let next = offer_and_lease(&mut publisher, &world);
    let short = Gap::between(&first, &next).expect("gap");
    let dt = world.step_size().seconds_per_tick();
    assert_eq!(short.ticks, 1);
    assert_eq!(short.travel_bound, dt * 4.0);
    assert!(short.unambiguous);

    // `first` and the leases on epoch 2 keep two slots. The third takes
    // the next boundary and stays displayable, so every later one is busy.
    let hold = feed.viewer(2).expect("viewer");
    let held = (hold.latest().expect("lease"), next);
    for _ in 0..600 {
        step(&mut world);
        publisher.offer(&world).expect("offer");
    }
    assert_eq!(publisher.stats().busy, 599);
    assert_eq!(publisher.stats().pending, Some(world.tick()));
    drop(held);
    let shown = hold.latest().expect("lease");
    assert_eq!(
        shown.tick().get(),
        2,
        "the feed kept its last displayable epoch"
    );
    step(&mut world);
    let after = offer_and_lease(&mut publisher, &world);
    assert_eq!(after.prior().map(|p| p.id), Some(shown.id()));
    let long = Gap::between(&shown, &after).expect("gap");
    assert_eq!(long.ticks, 600);
    assert_eq!(long.travel_bound, 600.0 * dt * 4.0);
    // 600 ticks at 4 units/s and Δt = 1/128 reach 18.75 units: past half
    // the 32-unit world, so minimum-image interpolation could reverse
    // an organism's path.
    assert!(!long.unambiguous, "{long:?}");
    assert_eq!(
        long.unambiguous,
        2.0 * long.travel_bound * (1.0 + ROUNDING_MARGIN) < 32.0
    );

    assert!(matches!(
        Gap::between(&after, &shown),
        Err(CoreError::StateConflict { .. })
    ));
    assert!(matches!(
        Gap::between(&after, &after),
        Err(CoreError::StateConflict { .. })
    ));
    let other = found(
        &ecology(&[("world", "width", "64.0"), ("world", "grid_width", "64")]),
        Arrangement::FreeList,
    );
    let foreign = publisher_lease(&other);
    assert!(matches!(
        Gap::between(&first, &foreign),
        Err(CoreError::StateConflict { .. })
    ));
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn publisher_lease(world: &World) -> Lease {
    let publisher = publisher(world);
    let lease = publisher
        .feed()
        .viewer(1)
        .expect("viewer")
        .latest()
        .expect("lease");
    drop(publisher);
    lease
}

/// Selecting by identity finds an organism's row, and an identity that is
/// not alive has none.
#[test]
fn selection_is_by_identity() {
    let config = ecology(&[]);
    let world = found(&config, Arrangement::Compact);
    let lease = publisher_lease(&world);
    assert_eq!(lease.row(OrganismId::new(0)), Some(0));
    assert_eq!(lease.row(OrganismId::new(47)), Some(47));
    assert_eq!(lease.row(OrganismId::new(48)), None);
    assert_eq!(lease.id_at(48), None);
}
