//! FL-022 evidence (plan §12.3, §14.3): a material brush is a command like
//! an injection. Its canonical record is its own tag, the cell, the amount
//! and every direction component in channel order, and it round-trips; a
//! truncated or extended record is refused. The book admits a material with
//! a unit-ball direction, refuses one outside it, and applies it at its
//! boundary. The expected world comes from a world stepped directly with
//! the same intervention, never from the book.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

mod common;

use common::{SESSION, advance, at, command, ecology, found, inject, reference};
use flife_core::ids::CommandId;
use flife_core::{CoreError, Tick};
use flife_engine::{Intervention, MATERIAL_CHANNELS};
use flife_runtime::{Admission, Command, CommandBook, CommandStatus, Outcome};

fn material(cell: [u32; 2], amount: f64, entries: &[(usize, f64)]) -> Intervention {
    let mut direction = [0.0; MATERIAL_CHANNELS];
    for &(channel, value) in entries {
        direction[channel] = value;
    }
    Intervention::InjectMaterial {
        cell,
        amount,
        direction,
    }
}

/// The payload is `inject-material`, NUL, the cell, the amount's bits and
/// the 32 components' bits in channel order; it decodes to the same
/// command, and the direction is part of the content.
#[test]
fn a_material_record_is_its_tag_cell_amount_and_every_component() {
    let brushing = command(4, at(7), material([3, 9], 1.5, &[(0, 0.6), (31, -0.8)]));
    let bytes = brushing.canonical_bytes();
    let mut tail = b"inject-material\0".to_vec();
    tail.extend_from_slice(&3_u32.to_le_bytes());
    tail.extend_from_slice(&9_u32.to_le_bytes());
    tail.extend_from_slice(&1.5_f64.to_bits().to_le_bytes());
    for channel in 0..MATERIAL_CHANNELS {
        let component = match channel {
            0 => 0.6_f64,
            31 => -0.8,
            _ => 0.0,
        };
        tail.extend_from_slice(&component.to_bits().to_le_bytes());
    }
    assert!(bytes.ends_with(&tail));
    // The prefix is the injection's: the schema through the revision.
    let energy = command(4, at(7), inject([3, 9], 1.5)).canonical_bytes();
    let prefix = energy.len() - (b"inject-energy\0".len() + 4 + 4 + 8);
    assert_eq!(bytes[..prefix], energy[..prefix]);
    assert_eq!(bytes.len(), prefix + tail.len());
    assert_eq!(
        Command::from_canonical_bytes(&bytes).expect("decodes"),
        brushing
    );

    let swapped = command(4, at(7), material([3, 9], 1.5, &[(0, -0.8), (31, 0.6)]));
    let signed = command(4, at(7), material([3, 9], 1.5, &[(0, -0.0), (31, -0.8)]));
    let plain = command(4, at(7), material([3, 9], 1.5, &[(31, -0.8)]));
    assert_ne!(brushing.content_hash(), swapped.content_hash());
    assert_ne!(
        signed.content_hash(),
        plain.content_hash(),
        "every bit of the direction is content"
    );
    assert_ne!(
        command(4, at(7), material([3, 9], 1.5, &[])).content_hash(),
        command(4, at(7), inject([3, 9], 1.5)).content_hash(),
        "a material is not an injection"
    );
}

/// A record missing its last component's last byte, with a byte more, or
/// with its tag misspelled is refused.
#[test]
fn truncated_extended_or_misnamed_material_records_are_refused() {
    let bytes = command(2, at(5), material([1, 1], 2.0, &[(4, 1.0)])).canonical_bytes();
    let mut extended = bytes.clone();
    extended.push(0);
    let tag = bytes
        .windows(b"inject-material".len())
        .position(|window| window == b"inject-material")
        .expect("the tag");
    let mut misnamed = bytes.clone();
    misnamed[tag + b"inject-".len()] = b'M';
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

/// The book refuses a material outside the unit ball, admits one inside it
/// for an exact boundary and applies it there: the outcome is the energy
/// the cell took, and the world equals one stepped directly with the same
/// material.
#[test]
fn the_book_applies_a_material_at_its_boundary() {
    let config = ecology(&[]);
    let mut world = found(&config);
    let grid = *world.field().grid();
    let mut book = CommandBook::new(SESSION, config.config_hash(), 16);
    assert!(matches!(
        book.admit(
            command(
                1,
                at(9),
                material([8, 8], 5.0, &[(0, 0.8), (1, 0.6), (2, 0.01)])
            ),
            world.tick(),
            &grid,
            false
        ),
        Admission::Rejected(_)
    ));
    let intervention = material([8, 8], 5.0, &[(0, 0.8), (2, -0.6)]);
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
            reported = Some(applied.applied);
        }
    }
    let reported = reported.expect("applied at tick 9");
    assert!(reported <= 5.0 && 5.0 - reported < 1.0e-5, "{reported}");
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
