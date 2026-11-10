//! FL-022 evidence (plan §12.3, §14.3): a removal is a command like an
//! injection. Its canonical record round-trips, its tags are checked, and
//! an injection's record is byte for byte what FL-014 wrote, so journals
//! and checkpoints written before removals existed still decode. The book
//! admits, seals and applies a removal, and its applied outcome is the
//! energy of the organisms it removed. The expected world comes from a
//! world stepped directly with the same intervention, never from the book.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

mod common;

use common::{SESSION, advance, at, command, ecology, found, inject, reference};
use flife_core::ids::CommandId;
use flife_core::{CoreError, Tick};
use flife_engine::{DeathCause, Disposition, Intervention};
use flife_runtime::{Admission, Command, CommandBook, CommandStatus, Outcome, Target};

fn removal(center: [f64; 2], radius: f64, disposition: Disposition) -> Intervention {
    Intervention::RemoveOrganisms {
        center,
        radius,
        disposition,
    }
}

/// The record FL-014 defined for an injection, written out by hand: the
/// schema and NUL, the session and identity, the target, the absent
/// revision, then `inject-energy`, NUL, the cell and the amount's bits.
#[test]
fn an_injection_keeps_its_record_and_a_removal_round_trips_its_own() {
    let injection = command(9, at(12), inject([5, 6], 2.5));
    let mut expected = b"frankenlife.command.v1\0".to_vec();
    expected.extend_from_slice(&SESSION.get().to_le_bytes());
    expected.extend_from_slice(&9_u64.to_le_bytes());
    expected.push(1);
    expected.extend_from_slice(&12_u64.to_le_bytes());
    expected.push(0);
    expected.push(0);
    expected.extend_from_slice(b"inject-energy\0");
    expected.extend_from_slice(&5_u32.to_le_bytes());
    expected.extend_from_slice(&6_u32.to_le_bytes());
    expected.extend_from_slice(&2.5_f64.to_bits().to_le_bytes());
    assert_eq!(injection.canonical_bytes(), expected);
    assert_eq!(
        Command::from_canonical_bytes(&expected).expect("decodes"),
        injection
    );

    let mut hashes = Vec::new();
    for disposition in Disposition::ALL {
        let removing = command(10, at(12), removal([31.5, 0.25], 4.0, disposition));
        let bytes = removing.canonical_bytes();
        let mut tail = b"remove-organisms\0".to_vec();
        tail.extend_from_slice(&31.5_f64.to_bits().to_le_bytes());
        tail.extend_from_slice(&0.25_f64.to_bits().to_le_bytes());
        tail.extend_from_slice(&4.0_f64.to_bits().to_le_bytes());
        tail.extend_from_slice(disposition.name().as_bytes());
        tail.push(0);
        assert!(bytes.ends_with(&tail), "{disposition:?}");
        assert_eq!(
            Command::from_canonical_bytes(&bytes).expect("decodes"),
            removing
        );
        hashes.push(removing.content_hash());
    }
    hashes.sort_by_key(|hash| *hash.as_bytes());
    hashes.dedup();
    assert_eq!(hashes.len(), 3, "the disposition is part of the content");
}

/// An unknown disposition or payload, a tag that never ends, trailing
/// bytes and a truncated record are refused.
#[test]
fn records_with_unknown_or_unterminated_tags_are_refused() {
    let bytes = command(1, at(3), removal([1.0, 2.0], 3.0, Disposition::Recycle)).canonical_bytes();
    let replaced = |from: &[u8], to: &[u8]| {
        let at = bytes
            .windows(from.len())
            .position(|window| window == from)
            .expect("present");
        let mut changed = bytes[..at].to_vec();
        changed.extend_from_slice(to);
        changed.extend_from_slice(&bytes[at + from.len()..]);
        changed
    };
    let mut trailing = bytes.clone();
    trailing.push(0);
    for refused in [
        replaced(b"recycle\0", b"erase\0"),
        replaced(b"remove-organisms\0", b"remove-particles\0"),
        replaced(b"recycle\0", &[b'r'; 40]),
        trailing,
        bytes[..bytes.len() - 1].to_vec(),
    ] {
        assert!(
            matches!(
                Command::from_canonical_bytes(&refused),
                Err(CoreError::IncompatibleCheckpoint { .. })
            ),
            "{:?}",
            String::from_utf8_lossy(&refused)
        );
    }
}

/// The book admits a removal for an exact boundary and applies it there.
/// Its applied outcome is the energy the removed organisms held, and the
/// world equals one stepped directly with the same removal.
#[test]
fn the_book_applies_a_removal_at_its_boundary() {
    let config = ecology(&[]);
    let mut world = found(&config);
    let grid = *world.field().grid();
    let mut book = CommandBook::new(SESSION, config.config_hash(), 16);
    let intervention = removal([16.0, 16.0], 10.0, Disposition::Dissipate);
    let admitted = book.admit(command(1, at(20), intervention), world.tick(), &grid, false);
    assert!(
        matches!(admitted, Admission::Accepted { retry: false, .. }),
        "{admitted:?}"
    );
    let mut applied = None;
    while world.tick().get() < 30 {
        let report = advance(&mut world, &mut book);
        if report.from == Tick::new(20) {
            let removed: f64 = report
                .deaths
                .iter()
                .filter(|death| death.cause == DeathCause::Removed(Disposition::Dissipate))
                .map(|death| death.residual)
                .sum();
            assert!(
                report.interventions[0].removed > 0,
                "the disk holds organisms"
            );
            applied = Some((report.epoch, removed));
        }
    }
    let (epoch, removed) = applied.expect("applied at tick 20");
    let Some(CommandStatus::Terminal(Outcome::Applied {
        tick,
        published,
        epoch: published_epoch,
        applied,
        ..
    })) = book.status(CommandId::new(1)).cloned()
    else {
        panic!("applied");
    };
    assert_eq!(
        (tick, published, published_epoch),
        (Tick::new(20), Tick::new(21), epoch)
    );
    assert!(
        (applied - removed).abs() <= 1.0e-12 * removed,
        "{applied} vs {removed}"
    );
    assert_eq!(
        world.digest(),
        reference(&config, 30, &[(20, intervention)]).digest()
    );
    assert!(matches!(
        book.admit(
            command(
                2,
                Target::NextAdmissible,
                removal([16.0, 16.0], 16.5, Disposition::Remove)
            ),
            world.tick(),
            &grid,
            false
        ),
        Admission::Rejected(_)
    ));
}
