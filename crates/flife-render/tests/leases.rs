//! FL-017 evidence (plan §9.5, §12.4, B.3, INV-10): the slot and lease
//! protocol. A leased or displayable epoch is never overwritten, a busy
//! feed skips and keeps the last displayable epoch, leases are bounded per
//! consumer, identities are never reissued, and publishing never touches
//! the world.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use common::{ecology, found, step};
use flife_core::CoreError;
use flife_core::ids::RenderEpoch;
use flife_engine::Arrangement;
use flife_render::{DISPLAY_PAIR, Epoch, Prior, Publication, Publisher, Viewer};

/// A leased epoch keeps its content while the world moves on and the feed
/// publishes around it. When every slot is leased or displayable the
/// boundary stays pending and the last displayable epoch stays; a release
/// lets the next offer publish the newest state, linked to the epoch it
/// replaced. Identities are issued once, in order.
#[test]
fn leased_epochs_are_never_overwritten_and_a_busy_feed_skips() {
    let config = ecology(&[]);
    let mut world = found(&config, Arrangement::FreeList);
    let releases = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&releases);
    let mut publisher = Publisher::new(&world, 3, move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })
    .expect("publisher");
    let feed = publisher.feed();
    assert_eq!(feed.capacity(), 3);
    assert_eq!(publisher.free(), 2, "the founding epoch is displayable");
    let a = feed.viewer(DISPLAY_PAIR).expect("viewer");
    let b = feed.viewer(1).expect("viewer");

    let one = a.latest().expect("lease");
    let one_copy: Epoch = one.clone();
    step(&mut world);
    let published = publisher.offer(&world).expect("offer");
    assert_eq!(
        published,
        Publication::Published {
            epoch: Prior {
                id: RenderEpoch::new(2),
                tick: world.tick()
            },
            replaced: Prior {
                id: RenderEpoch::new(1),
                tick: one.tick()
            },
        }
    );
    assert_eq!(
        publisher.offer(&world).expect("offer"),
        Publication::Current(publisher.latest()),
        "an unchanged world is not published twice"
    );
    let two = a.latest().expect("lease");
    let two_copy: Epoch = two.clone();
    let also_two = b.latest().expect("lease");
    assert_eq!(also_two.id(), two.id());
    step(&mut world);
    let three = publisher.offer(&world).expect("offer");
    let Publication::Published { epoch: shown, .. } = three else {
        panic!("{three:?}")
    };
    assert_eq!(publisher.free(), 0);

    let digest = world.digest();
    let mut busy = 0;
    for _ in 0..20 {
        step(&mut world);
        let skipped = publisher.offer(&world).expect("offer");
        assert_eq!(
            skipped,
            Publication::Busy {
                tick: world.tick(),
                kept: shown
            }
        );
        busy += 1;
        assert_eq!(feed.latest(), Some(shown));
        assert_eq!(*one, one_copy, "a leased epoch was overwritten");
        assert_eq!(*two, two_copy, "a leased epoch was overwritten");
    }
    assert_ne!(world.digest(), digest);
    assert_eq!(publisher.stats().busy, busy);
    assert_eq!(publisher.stats().published, 3);
    assert_eq!(publisher.stats().pending, Some(world.tick()));

    assert_eq!(releases.load(Ordering::SeqCst), 0);
    drop(also_two);
    assert_eq!(releases.load(Ordering::SeqCst), 1);
    assert_eq!(publisher.free(), 0, "epoch 2 is still leased by `a`");
    drop(one);
    assert_eq!(releases.load(Ordering::SeqCst), 2);
    assert_eq!(publisher.free(), 1);
    let Publication::Published { epoch, replaced } = publisher.offer(&world).expect("offer") else {
        panic!("not published")
    };
    assert_eq!(epoch.id, RenderEpoch::new(4));
    assert_eq!(
        epoch.tick,
        world.tick(),
        "the newest state, not the first skipped one"
    );
    assert_eq!(replaced, shown);
    assert_eq!(publisher.stats().pending, None);
    let four = b.latest().expect("lease");
    assert_eq!(four.prior(), Some(shown));
    assert_eq!(*two, two_copy);
    assert_eq!(publisher.next_id(), Some(5));
}

/// A dropped lease returns its slot reference and its budget before the
/// release hook runs. An owner woken by the hook, on any thread, then finds
/// the slot free.
#[test]
fn the_release_hook_runs_after_the_lease_is_returned() {
    let config = ecology(&[]);
    let world = found(&config, Arrangement::FreeList);
    let probe: Arc<Mutex<Option<Viewer>>> = Arc::new(Mutex::new(None));
    let seen: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let (hook_probe, hook_seen) = (Arc::clone(&probe), Arc::clone(&seen));
    let publisher = Publisher::new(&world, 3, move || {
        if let Some(viewer) = hook_probe.lock().unwrap().as_ref() {
            hook_seen.lock().unwrap().push(viewer.held());
        }
    })
    .expect("publisher");
    *probe.lock().unwrap() = Some(publisher.feed().viewer(2).expect("viewer"));
    let first = probe
        .lock()
        .unwrap()
        .as_ref()
        .expect("viewer")
        .latest()
        .expect("lease");
    let second = probe
        .lock()
        .unwrap()
        .as_ref()
        .expect("viewer")
        .latest()
        .expect("lease");
    drop(first);
    drop(second);
    assert_eq!(*seen.lock().unwrap(), vec![1, 0]);
    assert_eq!(publisher.free(), 2);
    *probe.lock().unwrap() = None;
}

/// Leases are bounded per consumer: a limit must leave a slot for the
/// publisher, a viewer cannot exceed its own limit, budgets are
/// independent, peeking takes nothing, and a lease outlives its viewer.
#[test]
fn lease_limits_are_per_consumer() {
    let config = ecology(&[]);
    let mut world = found(&config, Arrangement::FreeList);
    let mut publisher = Publisher::new(&world, 4, || {}).expect("publisher");
    let feed = publisher.feed();
    assert!(matches!(
        feed.viewer(0),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(matches!(
        feed.viewer(4),
        Err(CoreError::NumericalInvalid { .. })
    ));
    let viewer = feed.viewer(2).expect("viewer");
    assert_eq!(viewer.limit(), 2);
    let first = viewer.latest().expect("lease");
    let second = viewer.latest().expect("lease");
    assert_eq!(viewer.held(), 2);
    assert_eq!(
        viewer.latest().map(|_| ()),
        Err(CoreError::BudgetExhausted {
            budget: "render leases",
            required: 3,
            available: 2
        })
    );
    let other = feed.viewer(3).expect("viewer");
    let third = other.latest().expect("an independent budget");
    assert_eq!(other.held(), 1);
    drop(second);
    assert_eq!(viewer.held(), 1);
    assert!(viewer.latest_after(first.id()).expect("peek").is_none());
    assert_eq!(viewer.held(), 1, "peeking at an old epoch takes nothing");
    step(&mut world);
    publisher.offer(&world).expect("offer");
    let newer = viewer
        .latest_after(first.id())
        .expect("peek")
        .expect("a newer epoch");
    assert_eq!(newer.id().get(), 2);
    assert_eq!(viewer.held(), 2);
    drop(viewer);
    assert_eq!(newer.tick(), world.tick(), "a lease outlives its viewer");
    drop((first, third, newer));
    assert_eq!(publisher.free(), 3);
}

/// Fewer than two slots cannot hold a displayable epoch and one being
/// written. An exhausted identity sequence is refused at the next
/// publication and recorded, the last epoch stays, and the world is not
/// touched.
#[test]
fn refusals_never_touch_the_world_or_the_displayable_epoch() {
    let config = ecology(&[]);
    let mut world = found(&config, Arrangement::FreeList);
    assert!(matches!(
        Publisher::new(&world, 1, || {}),
        Err(CoreError::InvalidConfig(issues)) if issues[0].path == "execution.render_queue_capacity"
    ));
    assert!(matches!(
        Publisher::continuing(&world, 3, None, || {}),
        Err(CoreError::Overflow { .. })
    ));
    let mut publisher = Publisher::continuing(&world, 3, Some(u64::MAX), || {}).expect("publisher");
    let last = publisher.latest();
    assert_eq!(last.id, RenderEpoch::new(u64::MAX));
    step(&mut world);
    let digest = world.digest();
    assert_eq!(
        publisher.offer(&world),
        Err(CoreError::Overflow {
            quantity: "render epoch"
        })
    );
    assert_eq!(world.digest(), digest);
    assert_eq!(publisher.feed().latest(), Some(last));
    assert_eq!(publisher.latest(), last);
    assert_eq!(
        publisher.stats().failure,
        Some(CoreError::Overflow {
            quantity: "render epoch"
        })
    );
    assert_eq!(publisher.stats().pending, Some(world.tick()));
    assert_eq!(publisher.free(), 2, "the refused projection holds no slot");
}

/// Concurrent consumers on other threads read only complete, unchanging
/// epochs while the owner publishes. Every offer is published or busy, the
/// budgets return to zero, and the feed reports its closure.
#[test]
fn concurrent_viewers_read_only_complete_unchanging_epochs() {
    let config = ecology(&[]);
    let mut world = found(&config, Arrangement::FreeList);
    let mut publisher = Publisher::new(&world, 3, || {}).expect("publisher");
    let feed = publisher.feed();
    let done = Arc::new(AtomicBool::new(false));
    let readers: Vec<_> = (0..2)
        .map(|_| {
            let viewer = feed.viewer(1).expect("viewer");
            let done = Arc::clone(&done);
            thread::spawn(move || {
                let mut reads = 0_u64;
                let mut last = RenderEpoch::new(0);
                while !done.load(Ordering::Acquire) {
                    let Some(lease) = viewer.latest_after(last).expect("lease") else {
                        thread::yield_now();
                        continue;
                    };
                    lease.check().expect("a complete epoch");
                    let copy: Epoch = lease.clone();
                    thread::yield_now();
                    assert_eq!(*lease, copy, "an epoch changed while leased");
                    assert!(lease.id() > last);
                    last = lease.id();
                    reads += 1;
                }
                (reads, viewer.held())
            })
        })
        .collect();
    let (mut published, mut busy) = (0, 0);
    for _ in 0..200 {
        step(&mut world);
        match publisher.offer(&world).expect("offer") {
            Publication::Published { .. } => published += 1,
            Publication::Busy { .. } => busy += 1,
            Publication::Current(_) => panic!("the world changed"),
        }
    }
    done.store(true, Ordering::Release);
    for reader in readers {
        let (reads, held) = reader.join().expect("reader");
        assert!(reads > 0);
        assert_eq!(held, 0);
    }
    assert_eq!(published + busy, 200);
    assert_eq!(publisher.stats().published, published + 1);
    assert_eq!(publisher.stats().busy, busy);
    assert!(!feed.is_closed());
    publisher.close();
    assert!(feed.is_closed());
    assert_eq!(feed.latest(), Some(publisher.latest()));
}
