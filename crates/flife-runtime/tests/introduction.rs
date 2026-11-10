//! FL-022 evidence (plan §12.3, §14.3): a seeded introduction is a command
//! like an injection. Its canonical record is its own tag, the cell, the
//! count and the energy, and it round-trips; a truncated, extended or
//! misnamed record is refused. The book admits an introduction within the
//! rules, refuses one outside them, and applies it at its boundary. The
//! expected world comes from a world stepped directly with the same
//! intervention, never from the book.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

mod common;

use common::{SESSION, advance, at, command, ecology, found, inject, reference};
use flife_core::ids::CommandId;
use flife_core::{CoreError, Tick};
use flife_engine::Intervention;
use flife_runtime::{Admission, Command, CommandBook, CommandStatus, Outcome};

const fn introduce(cell: [u32; 2], count: u32, energy: f64) -> Intervention {
    Intervention::IntroduceOrganisms {
        cell,
        count,
        energy,
    }
}

/// The payload is `introduce-organisms`, NUL, the cell, the count and the
/// energy's bits; it decodes to the same command, and every field is
/// content.
#[test]
fn an_introduction_record_is_its_tag_cell_count_and_energy() {
    let introducing = command(4, at(7), introduce([3, 9], 12, 1.5));
    let bytes = introducing.canonical_bytes();
    let mut tail = b"introduce-organisms\0".to_vec();
    tail.extend_from_slice(&3_u32.to_le_bytes());
    tail.extend_from_slice(&9_u32.to_le_bytes());
    tail.extend_from_slice(&12_u32.to_le_bytes());
    tail.extend_from_slice(&1.5_f64.to_bits().to_le_bytes());
    assert!(bytes.ends_with(&tail));
    // The prefix is the injection's: the schema through the revision.
    let energy = command(4, at(7), inject([3, 9], 1.5)).canonical_bytes();
    let prefix = energy.len() - (b"inject-energy\0".len() + 4 + 4 + 8);
    assert_eq!(bytes[..prefix], energy[..prefix]);
    assert_eq!(bytes.len(), prefix + tail.len());
    assert_eq!(
        Command::from_canonical_bytes(&bytes).expect("decodes"),
        introducing
    );
    let hash = introducing.content_hash();
    for other in [
        introduce([9, 3], 12, 1.5),
        introduce([3, 9], 13, 1.5),
        introduce([3, 9], 12, 1.25),
    ] {
        assert_ne!(command(4, at(7), other).content_hash(), hash, "{other:?}");
    }
}

/// A record missing its energy's last byte, with a byte more, or with its
/// tag misspelled is refused.
#[test]
fn truncated_extended_or_misnamed_introduction_records_are_refused() {
    let bytes = command(2, at(5), introduce([1, 1], 2, 2.0)).canonical_bytes();
    let mut extended = bytes.clone();
    extended.push(0);
    let tag = bytes
        .windows(b"introduce-organisms".len())
        .position(|window| window == b"introduce-organisms")
        .expect("the tag");
    let mut misnamed = bytes.clone();
    misnamed[tag] = b'I';
    for refused in [bytes[..bytes.len() - 1].to_vec(), extended, misnamed] {
        assert!(
            matches!(
                Command::from_canonical_bytes(&refused),
                Err(CoreError::IncompatibleCheckpoint { .. })
            ),
            "{} bytes",
            refused.len()
        );
    }
}

/// The book refuses an introduction of no organisms, admits one within the
/// rules for an exact boundary and applies it there: the outcome is the
/// endowments it added, and the world equals one stepped directly with the
/// same introduction.
#[test]
fn the_book_applies_an_introduction_at_its_boundary() {
    let config = ecology(&[]);
    let mut world = found(&config);
    let grid = *world.field().grid();
    let mut book = CommandBook::new(SESSION, config.config_hash(), 16);
    assert!(matches!(
        book.admit(
            command(1, at(9), introduce([8, 8], 0, 5.0)),
            world.tick(),
            &grid,
            false
        ),
        Admission::Rejected(_)
    ));
    let intervention = introduce([8, 8], 7, 2.25);
    let admitted = book.admit(command(2, at(9), intervention), world.tick(), &grid, false);
    assert!(
        matches!(admitted, Admission::Accepted { retry: false, .. }),
        "{admitted:?}"
    );
    let mut reported = None;
    while world.tick().get() < 20 {
        let report = advance(&mut world, &mut book);
        if report.from == Tick::new(9) {
            let [applied] = report.interventions.as_slice() else {
                panic!("one intervention: {:?}", report.interventions);
            };
            assert_eq!(applied.intervention, intervention);
            assert_eq!(applied.introduced, 7);
            reported = Some(applied.applied);
        }
    }
    let reported = reported.expect("applied at tick 9");
    assert_eq!(reported.to_bits(), (7.0_f64 * 2.25).to_bits());
    let Some(CommandStatus::Terminal(Outcome::Applied { tick, applied, .. })) =
        book.status(CommandId::new(2)).cloned()
    else {
        panic!("applied");
    };
    assert_eq!(tick, Tick::new(9));
    assert_eq!(applied.to_bits(), reported.to_bits());
    assert_eq!(
        world.digest(),
        reference(&config, 20, &[(9, intervention)]).digest()
    );
}
