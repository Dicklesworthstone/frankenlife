//! FL-024's persistent telemetry (plan §14.1) in render epochs.
//!
//! - Every epoch carries its world's own quantities at its tick: the live
//!   population's logical parameters, the genome quota's bytes, the run
//!   ledger's report and whether a fault stopped the run. Each is compared
//!   with the world it was projected from.
//! - Every epoch a publisher publishes carries its tally of the ticks its
//!   owner counted: births, introductions, deaths, controller evaluations,
//!   rejected births and the energy organisms gained by uptake. It is
//!   compared with an independent sum over the ticks' reports, through
//!   boundaries the publisher skipped while every slot was held, and with
//!   the world's own event cursor and identity sequence.
//! - A tick not counted, or counted twice, ends the tally: no later epoch
//!   carries one. A tick offered before it is counted carries none, and the
//!   tally goes on. An epoch outside a feed carries none.
//! - An epoch of a world a numerical fault stopped says so.

mod common;

use common::{ecology, found, step};
use flife_core::{Flow, Tick};
use flife_engine::{Arrangement, Disposition, Intervention, Stepped, TickReport, World};
use flife_render::{Epoch, Lease, Publication, Publisher, Tally};

/// The ticks the long run steps.
const TICKS: u64 = 160;

/// `report` added to `tally`, written apart from the crate's own sum.
fn counted(tally: &mut Tally, report: &TickReport) {
    tally.deaths += u64::try_from(report.deaths.len()).unwrap_or(u64::MAX);
    tally.births += u64::try_from(report.births.len()).unwrap_or(u64::MAX);
    tally.introductions += u64::try_from(report.introductions.len()).unwrap_or(u64::MAX);
    tally.rejected_births += u64::try_from(report.rejected.len()).unwrap_or(u64::MAX);
    tally.evaluations += u64::try_from(report.evaluated).unwrap_or(u64::MAX);
    tally.uptake += report.closure.credited(flife_core::Transfer::Uptake);
}

/// `epoch`'s world quantities are `world`'s, and its tally is `tally`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn assert_telemetry(epoch: &Epoch, world: &World, tally: Option<Tally>) {
    let metrics = epoch.metrics();
    assert_eq!(metrics.tick, world.tick());
    let population = world.population();
    let parameters: u64 = population
        .iter()
        .map(|organism| {
            let genome = population.genome(organism.id).expect("a genome");
            u64::try_from(genome.parameters().len()).expect("a count")
        })
        .sum();
    assert!(parameters > 0, "a live population has parameters");
    assert_eq!(
        metrics.parameters,
        parameters,
        "tick {}",
        world.tick().get()
    );
    assert_eq!(metrics.genome_logical_bytes, world.quota().logical_bytes());
    assert_eq!(
        metrics.genome_allocated_bytes,
        world.quota().allocated_bytes()
    );
    assert!(metrics.genome_logical_bytes > 0 && metrics.genome_allocated_bytes > 0);
    assert_eq!(metrics.ledger, world.ledger().report());
    assert_eq!(metrics.faulted, world.fault().is_some());
    assert_eq!(metrics.tally, tally, "tick {}", world.tick().get());
    assert_eq!(
        metrics.tally.map(|tally| tally.uptake.to_bits()),
        tally.map(|tally| tally.uptake.to_bits())
    );
}

#[test]
fn every_epoch_carries_its_worlds_telemetry_and_the_tally_through_its_tick() {
    let config = ecology(&[]);
    let mut world = found(&config, Arrangement::FreeList);
    let mut publisher = Publisher::new(&world, 3, || {}).expect("a publisher");
    let feed = publisher.feed();
    // One viewer reads each new epoch; the other holds two at a time.
    let viewer = feed.viewer(1).expect("a viewer");
    let holder = feed.viewer(2).expect("a viewer");
    let mut expected = Tally::from(world.tick());
    assert_eq!(expected.since, Tick::ZERO);
    let founding = viewer.latest().expect("the founding epoch");
    assert_telemetry(&founding, &world, Some(expected));
    drop(founding);

    // Σ (granted − what uptake's inefficiency dissipated), and Σ granted.
    let (mut gained, mut granted) = (0.0_f64, 0.0_f64);
    let (mut published, mut busy) = (0_u64, 0_u64);
    let mut held: Vec<Lease> = Vec::new();
    for tick in 0..TICKS {
        let interventions = match tick {
            40 => vec![Intervention::IntroduceOrganisms {
                cell: [3, 29],
                count: 5,
                energy: 2.0,
            }],
            90 => vec![Intervention::RemoveOrganisms {
                center: [16.0, 16.0],
                radius: 6.0,
                disposition: Disposition::Remove,
            }],
            _ => Vec::new(),
        };
        let report = match world.step_with(&interventions).expect("the tick runs") {
            Stepped::Published(report) => *report,
            Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
        };
        counted(&mut expected, &report);
        granted += report.uptake.granted;
        gained += report.uptake.granted - report.closure.flow(Flow::UptakeInefficiency);
        publisher.count(&report);
        match publisher.offer(&world).expect("an offer") {
            Publication::Published { .. } => {
                published += 1;
                let lease = viewer.latest().expect("the new epoch");
                assert_telemetry(&lease, &world, Some(expected));
                drop(lease);
                // Hold two epochs for a while: every slot is then busy, and
                // the boundaries between are counted but not published.
                if tick % 16 < 2 {
                    held.push(holder.latest().expect("the new epoch"));
                }
            }
            Publication::Busy { .. } => busy += 1,
            Publication::Current(prior) => panic!("tick {tick} was already shown: {prior:?}"),
        }
        if tick % 16 == 5 {
            held.clear();
        }
    }
    assert_eq!(publisher.tally(), Some(expected));
    println!(
        "{TICKS} ticks: {published} published, {busy} skipped; {expected:?}; {} live",
        world.population().len()
    );
    assert!(
        busy > 0 && published > 0,
        "{busy} skipped, {published} published"
    );
    assert!(expected.births > 0 && expected.deaths > 0, "{expected:?}");
    assert_eq!(expected.introductions, 5);
    assert!(
        expected.evaluations > 0 && expected.uptake > 0.0,
        "{expected:?}"
    );
    // The world's own counters agree: its event cursor counts every death,
    // birth and introduction, and every identity it issued is live or dead.
    assert_eq!(
        world.events(),
        expected.deaths + expected.births + expected.introductions
    );
    assert_eq!(
        world.identities().organisms.next_raw(),
        Some(u64::try_from(world.population().len()).expect("a count") + expected.deaths)
    );
    // The uptake counted is what organisms gained: what was granted, less
    // what their efficiency dissipated (to rounding), not the grant itself.
    assert!(
        (expected.uptake - gained).abs() <= 1e-9 * granted,
        "{} gained, {gained} from the grants",
        expected.uptake
    );
    assert!(
        expected.uptake < granted,
        "{} of {granted}",
        expected.uptake
    );
}

#[test]
fn a_tick_not_counted_or_counted_twice_ends_the_tally() {
    let config = ecology(&[]);
    let mut world = found(&config, Arrangement::FreeList);
    // A publisher opened on a world three ticks in counts from there.
    for _ in 0..3 {
        let _ = step(&mut world);
    }
    let mut publisher = Publisher::new(&world, 3, || {}).expect("a publisher");
    let viewer = publisher.feed().viewer(1).expect("a viewer");
    let mut expected = Tally::from(Tick::new(3));
    assert_eq!(publisher.tally(), Some(expected));
    let report = step(&mut world);
    counted(&mut expected, &report);
    publisher.count(&report);
    publisher.offer(&world).expect("an offer");
    assert_telemetry(&viewer.latest().expect("a lease"), &world, Some(expected));

    // A tick offered before it is counted: its epoch carries none, since
    // the tally stops a tick short of it. Counted afterwards, the tally goes
    // on, and the next epoch carries it whole.
    let report = step(&mut world);
    publisher.offer(&world).expect("an offer");
    assert_telemetry(&viewer.latest().expect("a lease"), &world, None);
    counted(&mut expected, &report);
    publisher.count(&report);
    assert_eq!(publisher.tally(), Some(expected));
    let report = step(&mut world);
    counted(&mut expected, &report);
    publisher.count(&report);
    publisher.offer(&world).expect("an offer");
    assert_telemetry(&viewer.latest().expect("a lease"), &world, Some(expected));

    // One tick not counted: this epoch and every later one carry none.
    let _missed = step(&mut world);
    for _ in 0..2 {
        let report = step(&mut world);
        publisher.count(&report);
        publisher.offer(&world).expect("an offer");
        assert_telemetry(&viewer.latest().expect("a lease"), &world, None);
        assert_eq!(publisher.tally(), None);
    }

    // One tick counted twice.
    let mut world = found(&config, Arrangement::FreeList);
    let mut publisher = Publisher::new(&world, 3, || {}).expect("a publisher");
    let viewer = publisher.feed().viewer(1).expect("a viewer");
    let report = step(&mut world);
    publisher.count(&report);
    publisher.count(&report);
    publisher.offer(&world).expect("an offer");
    assert_telemetry(&viewer.latest().expect("a lease"), &world, None);

    // An epoch outside a feed has the world's quantities and no tally.
    let standalone = Epoch::standalone(&world).expect("an epoch");
    assert_telemetry(&standalone, &world, None);
}

#[test]
fn an_epoch_of_a_faulted_world_says_a_fault_stopped_it() {
    // One source strong enough that a tick's arithmetic leaves the finite
    // range, as flife-runtime's stop-and-preserve test does.
    let config = ecology(&[
        ("sources", "count", "1"),
        ("sources", "power_per_source", "1.0e40"),
        ("sources", "sigma_world_units", "0.5"),
    ]);
    let mut world = found(&config, Arrangement::FreeList);
    let before = Epoch::standalone(&world).expect("an epoch");
    assert!(!before.metrics().faulted);
    assert_telemetry(&before, &world, None);
    let faulted_at = (0..200).find(|_| world.step_with(&[]).is_err());
    assert!(
        faulted_at.is_some() && world.fault().is_some(),
        "the run faults within 200 ticks"
    );
    let after = Epoch::standalone(&world).expect("an epoch");
    assert!(after.metrics().faulted, "{faulted_at:?}");
    assert_telemetry(&after, &world, None);
}
