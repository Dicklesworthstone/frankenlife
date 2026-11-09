//! FL-017 evidence (plan §7.5, §12.4, G3, INV-09, Appendix C.14):
//! observation is state-neutral under retained-lease pressure.
//!
//! A session publishes a render epoch at every committed boundary. Viewers
//! that retain leases, cycle them slowly, or read from another thread can
//! cost display freshness only. Every committed world is compared with one
//! stepped directly, with no session and no feed. A near-capacity birth
//! tick makes the same admission decision whether readers are idle,
//! retaining or slow.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use asupersync::runtime::{RuntimeBuilder, yield_now};
use common::{Client, SESSION, cx, ecology, found, lab, publish, reference};
use flife_core::canonical::Sha256Digest;
use flife_core::ids::RenderEpoch;
use flife_core::{Tick, ValidatedConfig};
use flife_engine::{Advance, CapacityShortfall};
use flife_render::{DISPLAY_PAIR, Feed};
use flife_runtime::{Boundary, RunEnd, StepOutcome, open};

/// The digest and birth and death counts of `config`'s world at every
/// tick up to `ticks`, stepped directly.
fn trajectory(config: &ValidatedConfig, ticks: u64) -> Vec<(Sha256Digest, usize, usize)> {
    let mut world = found(config);
    let mut states = vec![(world.digest(), 0, 0)];
    for _ in 0..ticks {
        let report = publish(&mut world, &[]);
        states.push((world.digest(), report.births.len(), report.deaths.len()));
    }
    states
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

/// G3, INV-09: a viewer that retains its leases makes the feed skip, keep
/// its last displayable epoch and report the pending boundary. Every step
/// still commits exactly the directly stepped world, births and deaths
/// included. Releasing the leases while paused wakes the owner, which
/// publishes the newest boundary without advancing. A later run under
/// renewed pressure reaches the same world as the direct trajectory.
#[test]
fn retained_leases_skip_epochs_but_never_change_the_world() {
    let config = ecology(&[]);
    let expected = trajectory(&config, 96);
    let final_digest = expected[96].0;
    for seed in [1, 2, 3] {
        let expected = expected.clone();
        let (results, closed) = lab(seed, &config, |handle| {
            vec![Box::pin(async move {
                let cx = cx();
                let feed = handle.feed();
                let hog = feed.viewer(DISPLAY_PAIR).expect("viewer");
                let first = hog.latest().expect("lease");
                assert_eq!(
                    (first.id(), first.tick()),
                    (RenderEpoch::new(1), Tick::new(0))
                );
                let mut kept = Vec::new();
                for step in 1..=48_u64 {
                    let outcome = handle.step(&cx).await.expect("step");
                    let StepOutcome::Published {
                        boundary, report, ..
                    } = outcome
                    else {
                        panic!("{outcome:?}")
                    };
                    let state = &expected[step as usize];
                    let snapshot = handle.inspect(&cx).await.expect("inspect");
                    assert_eq!(boundary.tick, Tick::new(step));
                    assert_eq!(snapshot.digest, state.0, "tick {step}");
                    assert_eq!(
                        (report.births.len(), report.deaths.len()),
                        (state.1, state.2)
                    );
                    if step == 1 {
                        // The hog's second lease: two slots held, one left.
                        kept.push(hog.latest().expect("lease"));
                    }
                    let latest = feed.latest().expect("an epoch");
                    if step <= 2 {
                        assert_eq!(latest.tick, Tick::new(step));
                    } else {
                        assert_eq!(latest.tick, Tick::new(2), "the last displayable epoch");
                        assert_eq!(snapshot.render.pending, Some(Tick::new(step)));
                        assert_eq!(snapshot.render.busy, step - 2);
                    }
                }
                let paused = handle.inspect(&cx).await.expect("inspect");
                drop((first, kept));
                assert!(
                    shows(&feed, Tick::new(48)).await,
                    "a release while paused publishes the pending boundary"
                );
                let after = handle.inspect(&cx).await.expect("inspect");
                assert_eq!(after.boundary, paused.boundary, "without a tick");
                assert_eq!(after.render.pending, None);
                assert_eq!(after.render.published, 4);

                // Pressure again: two distinct epochs held, so after one more
                // publication every slot is busy.
                let older = hog.latest().expect("lease");
                assert!(matches!(
                    handle.step(&cx).await.expect("step"),
                    StepOutcome::Published { .. }
                ));
                let newer = hog.latest().expect("lease");
                assert_eq!((older.tick(), newer.tick()), (Tick::new(48), Tick::new(49)));
                let end = handle.run_until(&cx, Tick::new(96)).await.expect("run");
                assert!(
                    matches!(end, RunEnd::Reached(Boundary { tick, .. }) if tick == Tick::new(96))
                );
                let finished = handle.inspect(&cx).await.expect("inspect");
                assert_eq!(finished.digest, expected[96].0);
                assert_eq!(feed.latest().map(|l| l.tick), Some(Tick::new(50)));
                assert_eq!(finished.render.busy, 46 + 46);
                drop((older, newer));
                handle.close(&cx).await.expect("close")
            }) as Client<Boundary>]
        });
        assert_eq!(results[0].tick, Tick::new(96));
        assert_eq!(closed.world.digest(), final_digest);
        assert_eq!(closed.render.failure, None);
        assert_eq!(closed.render.busy, 46 + 46, "{:?}", closed.render);
    }
}

/// The tick at which the population cap first pauses this world.
#[expect(
    clippy::panic,
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn first_capacity_pause(config: &ValidatedConfig) -> u64 {
    let mut world = found(config);
    for _ in 0..512 {
        let tick = world.tick().get();
        match world.advance().expect("advance") {
            Advance::Ready(candidate) => {
                candidate.publish().expect("publish");
            }
            Advance::Paused(_) => return tick,
        }
    }
    panic!("no capacity pause in 512 ticks");
}

/// How the readers behave in one C.14 run.
#[derive(Debug, Clone, Copy)]
enum Readers {
    /// No viewer.
    Idle,
    /// Viewers hold every slot the feed does not show, for the whole run.
    Retaining,
    /// A viewer task leases each new epoch and holds it across many yields.
    Slow,
}

/// One run to the capacity pause under `readers`: the run's end, a
/// retried step's outcome, and the world the session returns.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn capacity_run(
    seed: u64,
    config: &ValidatedConfig,
    pause: u64,
    readers: Readers,
) -> (RunEnd, StepOutcome, Sha256Digest) {
    let (results, closed) = lab(seed, config, move |handle| {
        let feed = handle.feed();
        let mut clients: Vec<Client<Option<(RunEnd, StepOutcome)>>> = Vec::new();
        clients.push(Box::pin(async move {
            let cx = cx();
            let feed = handle.feed();
            let a = feed.viewer(1).expect("viewer");
            let b = feed.viewer(1).expect("viewer");
            let held = match readers {
                Readers::Retaining => {
                    let old = a.latest().expect("lease");
                    let outcome = handle.step(&cx).await.expect("step");
                    assert!(matches!(outcome, StepOutcome::Published { .. }));
                    Some((old, b.latest().expect("lease")))
                }
                Readers::Idle | Readers::Slow => None,
            };
            let end = handle
                .run_until(&cx, Tick::new(pause + 8))
                .await
                .expect("run");
            let retry = handle.step(&cx).await.expect("step");
            drop(held);
            handle.close(&cx).await.expect("close");
            Some((end, retry))
        }));
        if matches!(readers, Readers::Slow) {
            clients.push(Box::pin(async move {
                let viewer = feed.viewer(DISPLAY_PAIR).expect("viewer");
                let mut last = RenderEpoch::new(0);
                let mut held = Vec::new();
                while !feed.is_closed() {
                    if let Some(lease) = viewer.latest_after(last).expect("lease") {
                        last = lease.id();
                        held.push(lease);
                        if held.len() == DISPLAY_PAIR {
                            held.remove(0);
                        }
                    }
                    for _ in 0..7 {
                        yield_now().await;
                    }
                }
                None
            }));
        }
        clients
    });
    let (end, retry) = results
        .into_iter()
        .flatten()
        .next()
        .expect("the controlling client");
    (end, retry, closed.world.digest())
}

/// C.14: a near-capacity birth tick with idle, retaining and slow readers.
/// Every run pauses at the same boundary with the same shortfall, the
/// identical retry pauses identically, and the session returns the same
/// world as one stepped directly to the pause. Only display freshness
/// differs.
#[test]
fn c14_slow_readers_on_a_near_capacity_birth_tick_change_no_admission_decision() {
    let config = ecology(&[("population", "maximum_count", "48")]);
    let pause = first_capacity_pause(&config);
    let expected = reference(&config, pause, &[]).digest();
    let mut decisions: Vec<(Boundary, CapacityShortfall)> = Vec::new();
    for (seed, readers) in [
        (11, Readers::Idle),
        (12, Readers::Retaining),
        (13, Readers::Slow),
        (14, Readers::Retaining),
        (15, Readers::Slow),
    ] {
        let (end, retry, digest) = capacity_run(seed, &config, pause, readers);
        let RunEnd::CapacityPaused {
            boundary,
            shortfall,
        } = end
        else {
            panic!("{readers:?}: {end:?}")
        };
        assert_eq!(boundary.tick, Tick::new(pause), "{readers:?}");
        assert_eq!(
            retry,
            StepOutcome::Paused {
                boundary,
                shortfall: shortfall.clone()
            },
            "{readers:?}"
        );
        assert_eq!(digest, expected, "{readers:?}");
        decisions.push((boundary, shortfall));
    }
    assert!(decisions.windows(2).all(|pair| pair[0] == pair[1]));
}

/// The native runtime with a reader on another thread, leasing every epoch
/// it can and checking each one, while the session runs: the world is the
/// directly stepped one, and the reader only ever sees complete epochs in
/// increasing order.
#[test]
fn a_reader_thread_cannot_change_the_world() {
    let config = ecology(&[]);
    let expected = reference(&config, 160, &[]).digest();
    let (owner, handle) = open(found(&config), &config, SESSION).expect("open");
    let feed = handle.feed();
    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let viewer = feed.viewer(DISPLAY_PAIR).expect("viewer");
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            let (mut last, mut reads, mut held) = (RenderEpoch::new(0), 0_u64, Vec::new());
            while !stop.load(Ordering::Acquire) {
                match viewer.latest_after(last).expect("lease") {
                    Some(lease) => {
                        lease.check().expect("a complete epoch");
                        assert!(lease.id() > last);
                        last = lease.id();
                        reads += 1;
                        held.push(lease);
                        if held.len() == DISPLAY_PAIR {
                            held.remove(0);
                        }
                    }
                    None => thread::yield_now(),
                }
            }
            reads
        })
    };
    let runtime = RuntimeBuilder::current_thread().build().expect("runtime");
    let spawner = runtime.handle();
    let closed = runtime.block_on(async move {
        let owner = spawner
            .try_spawn(async move {
                let cx = cx();
                owner.run(&cx).await
            })
            .expect("owner");
        let client = spawner
            .try_spawn(async move {
                let cx = cx();
                let end = handle.run_until(&cx, Tick::new(160)).await.expect("run");
                assert!(matches!(end, RunEnd::Reached(_)));
                handle.close(&cx).await.expect("close")
            })
            .expect("client");
        client.await;
        owner.await
    });
    stop.store(true, Ordering::Release);
    let reads = reader.join().expect("reader");
    assert!(reads > 0);
    assert_eq!(closed.world.digest(), expected);
    assert_eq!(closed.render.failure, None);
    assert!(feed.is_closed());
    // The last boundary is shown, or reported pending if the reader held
    // every other slot at that moment.
    let shown = feed.latest().map(|l| l.tick);
    assert!(
        shown == Some(Tick::new(160)) || closed.render.pending == Some(Tick::new(160)),
        "{shown:?} {:?}",
        closed.render
    );
}
