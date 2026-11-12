//! FL-024's persistent telemetry (plan §14.1) through a session: the owner
//! counts every tick it publishes, stepped or run, so every epoch its feed
//! publishes carries the tally through the epoch's own tick, including the
//! ticks it skipped while a viewer held every slot. Each tally is compared
//! with one summed over a world stepped directly with the same commands.

mod common;

use asupersync::runtime::yield_now;
use common::{Client, at, command, cx, ecology, found, lab, publish};
use flife_core::{Tick, Transfer};
use flife_engine::{Disposition, Intervention, TickReport};
use flife_render::{DISPLAY_PAIR, Feed, Tally};
use flife_runtime::{Admission, Boundary, RunEnd, StepOutcome};

/// The ticks the session reaches.
const TICKS: u64 = 96;

/// The introduction the session applies, at its tick.
const INTRODUCED_AT: u64 = 30;
const INTRODUCTION: Intervention = Intervention::IntroduceOrganisms {
    cell: [3, 29],
    count: 4,
    energy: 2.0,
};

/// The removal the session applies, at its tick: deaths.
const REMOVED_AT: u64 = 40;
const REMOVAL: Intervention = Intervention::RemoveOrganisms {
    center: [16.0, 16.0],
    radius: 12.0,
    disposition: Disposition::Remove,
};

/// `report` added to `tally`, written apart from the crate's own sum.
fn counted(mut tally: Tally, report: &TickReport) -> Tally {
    tally.deaths += report.deaths.len() as u64;
    tally.births += report.births.len() as u64;
    tally.introductions += report.introductions.len() as u64;
    tally.rejected_births += report.rejected.len() as u64;
    tally.evaluations += report.evaluated as u64;
    tally.uptake += report.closure.credited(Transfer::Uptake);
    tally
}

/// The tally at every tick up to [`TICKS`] of the world stepped directly
/// with [`INTRODUCTION`] and [`REMOVAL`] at their ticks.
fn tallies() -> Vec<Tally> {
    let mut world = found(&ecology(&[]));
    let mut tally = Tally::from(Tick::ZERO);
    let mut all = vec![tally];
    for tick in 0..TICKS {
        let due = match tick {
            INTRODUCED_AT => vec![INTRODUCTION],
            REMOVED_AT => vec![REMOVAL],
            _ => Vec::new(),
        };
        tally = counted(tally, &publish(&mut world, &due));
        all.push(tally);
    }
    all
}

/// The tick and tally of the epoch `feed` shows now.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn shown(feed: &Feed) -> (Tick, Option<Tally>) {
    let lease = feed.viewer(1).expect("a viewer").latest().expect("a lease");
    (lease.tick(), lease.metrics().tally)
}

/// Yield until `feed` shows `tick`, within a bound.
async fn shows(feed: &Feed, tick: Tick) -> bool {
    for _ in 0..1_000 {
        if feed.latest().map(|latest| latest.tick) == Some(tick) {
            return true;
        }
        yield_now().await;
    }
    false
}

#[test]
fn every_epoch_a_session_publishes_carries_the_tally_through_its_tick() {
    let config = ecology(&[]);
    let expected = tallies();
    let last = expected[TICKS as usize];
    assert!(
        last.births > 0 && last.deaths > 0 && last.introductions == 4,
        "{last:?}"
    );
    for seed in [1, 2] {
        let expected = expected.clone();
        let (results, closed) = lab(seed, &config, |handle| {
            vec![Box::pin(async move {
                let cx = cx();
                let feed = handle.feed();
                assert_eq!(shown(&feed), (Tick::ZERO, Some(expected[0])));
                for (id, tick, intervention) in
                    [(1, INTRODUCED_AT, INTRODUCTION), (2, REMOVED_AT, REMOVAL)]
                {
                    let admission = handle
                        .submit(&cx, command(id, at(tick), intervention))
                        .await
                        .expect("submits");
                    assert!(
                        matches!(admission, Admission::Accepted { .. }),
                        "{admission:?}"
                    );
                }
                let hog = feed.viewer(DISPLAY_PAIR).expect("a viewer");
                let mut held = Vec::new();
                let mut skipped = 0;
                for step in 1..=48_u64 {
                    let outcome = handle.step(&cx).await.expect("steps");
                    assert!(
                        matches!(outcome, StepOutcome::Published { .. }),
                        "{outcome:?}"
                    );
                    let (tick, tally) = shown(&feed);
                    assert_eq!(
                        tally,
                        Some(expected[tick.get() as usize]),
                        "step {step} shows tick {}",
                        tick.get()
                    );
                    if tick.get() < step {
                        skipped += 1;
                    }
                    // Two epochs held from step 10 to 20: the boundaries
                    // between are counted and not published.
                    if step == 10 || step == 11 {
                        held.push(hog.latest().expect("a lease"));
                    }
                    if step == 20 {
                        held.clear();
                    }
                }
                assert!(skipped > 0, "boundaries were skipped");
                assert!(shows(&feed, Tick::new(48)).await);
                assert_eq!(shown(&feed), (Tick::new(48), Some(expected[48])));

                // A run, with every slot held part of the way: the pending
                // boundary published on release carries the run's tally.
                held.push(hog.latest().expect("a lease"));
                let end = handle.run_until(&cx, Tick::new(60)).await.expect("runs");
                assert!(
                    matches!(end, RunEnd::Reached(Boundary { tick, .. }) if tick == Tick::new(60))
                );
                held.push(hog.latest().expect("a lease"));
                let end = handle.run_until(&cx, Tick::new(TICKS)).await.expect("runs");
                assert!(
                    matches!(end, RunEnd::Reached(Boundary { tick, .. }) if tick == Tick::new(TICKS))
                );
                let (tick, tally) = shown(&feed);
                assert!(tick.get() < TICKS, "every slot was held");
                assert_eq!(tally, Some(expected[tick.get() as usize]));
                drop(held);
                assert!(
                    shows(&feed, Tick::new(TICKS)).await,
                    "the release publishes"
                );
                let (tick, tally) = shown(&feed);
                assert_eq!(
                    (tick, tally),
                    (Tick::new(TICKS), Some(expected[TICKS as usize]))
                );
                let tally = tally.unwrap_or_else(|| panic!("a tally"));
                assert_eq!(
                    tally.uptake.to_bits(),
                    expected[TICKS as usize].uptake.to_bits()
                );
                handle.close(&cx).await.expect("closes")
            }) as Client<Boundary>]
        });
        assert_eq!(results[0].tick, Tick::new(TICKS));
        assert_eq!(closed.render.failure, None);
        assert!(closed.render.busy > 0, "{:?}", closed.render);
    }
}
