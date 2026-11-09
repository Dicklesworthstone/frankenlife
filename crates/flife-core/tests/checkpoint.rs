//! FL-016's checkpoint container (Appendix B.2): what the writer emits
//! parses back to the same chunks, and each manifest defect is refused by
//! name even when the header and manifest are re-sealed around it.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use flife_core::CoreError;
use flife_core::Sha256Digest;
use flife_core::checkpoint::{
    ByteReader, ByteWriter, CHECKPOINT_FORMAT, CheckpointReader, CheckpointWriter, ChunkFamily,
    MANIFEST_MAX_BYTES,
};

const MODEL: &str = "field-ecology-v1";

/// Three chunks: configuration, field (on configuration) and ledger (on
/// field).
fn sample() -> Vec<u8> {
    let mut writer = CheckpointWriter::new(MODEL);
    writer
        .chunk(ChunkFamily::Configuration, 1, 1, b"config".to_vec(), &[])
        .expect("first");
    writer
        .chunk(
            ChunkFamily::Field,
            3,
            4,
            vec![1, 2, 3, 4, 5, 6, 7, 8],
            &[ChunkFamily::Configuration],
        )
        .expect("second");
    writer
        .chunk(
            ChunkFamily::Ledger,
            1,
            1,
            b"ledger".to_vec(),
            &[ChunkFamily::Field],
        )
        .expect("third");
    writer.finish().expect("finishes")
}

/// Where the manifest starts and how long it is.
fn manifest(bytes: &[u8]) -> (usize, usize) {
    let model = usize::from(u16::from_le_bytes([bytes[13], bytes[14]]));
    let length = u32::from_le_bytes(bytes[15 + model..19 + model].try_into().expect("four"));
    (19 + model, usize::try_from(length).expect("small"))
}

/// Recompute the seal over the header and manifest.
fn reseal(bytes: &mut [u8]) {
    let (start, length) = manifest(bytes);
    let at = start + length;
    let seal = Sha256Digest::of(&bytes[..at]);
    bytes[at..at + 32].copy_from_slice(seal.as_bytes());
}

/// Offset of chunk `index`'s manifest entry. Entries after the first have
/// one dependency in [`sample`], the first none.
fn entry(bytes: &[u8], index: usize) -> usize {
    let (start, _) = manifest(bytes);
    let fixed = 1 + 1 + 2 + 8 + 8 + 8 + 8 + 32 + 1;
    let mut at = start + 2;
    for chunk in 0..index {
        at += fixed + usize::from(chunk > 0);
    }
    at
}

fn refusal(bytes: &[u8]) -> String {
    match CheckpointReader::parse(bytes, &[]) {
        Err(CoreError::IncompatibleCheckpoint { reason }) => reason,
        Err(other) => panic!("another error class: {other}"),
        Ok(_) => panic!("accepted a defective checkpoint"),
    }
}

#[test]
fn a_written_checkpoint_parses_back_to_its_chunks() {
    let bytes = sample();
    let reader = CheckpointReader::parse(
        &bytes,
        &[
            ChunkFamily::Configuration,
            ChunkFamily::Field,
            ChunkFamily::Ledger,
        ],
    )
    .expect("parses");
    assert_eq!(reader.model(), MODEL);
    assert_eq!(
        reader.families().collect::<Vec<_>>(),
        [
            ChunkFamily::Configuration,
            ChunkFamily::Field,
            ChunkFamily::Ledger
        ]
    );
    let field = reader.versioned(ChunkFamily::Field, 3).expect("the field");
    assert_eq!(
        (field.version, field.items, field.bytes),
        (3, 4, &[1, 2, 3, 4, 5, 6, 7, 8][..])
    );
    assert!(
        reader.versioned(ChunkFamily::Field, 1).is_err(),
        "another version"
    );
    assert!(reader.chunk(ChunkFamily::Genomes).is_err(), "absent");
    assert_eq!(&bytes[..8], b"FLIFECKP");
    assert_eq!(
        u32::from_le_bytes(bytes[8..12].try_into().expect("four")),
        CHECKPOINT_FORMAT
    );
    assert!(
        refusal_required(&bytes, ChunkFamily::Genomes).contains("missing chunk genomes"),
        "a required family that is absent"
    );
    for family in ChunkFamily::ALL {
        assert_eq!(ChunkFamily::from_code(family.code()), Some(family));
    }
    assert_eq!(ChunkFamily::from_code(0), None);
}

fn refusal_required(bytes: &[u8], family: ChunkFamily) -> String {
    match CheckpointReader::parse(bytes, &[family]) {
        Err(CoreError::IncompatibleCheckpoint { reason }) => reason,
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_writer_refuses_duplicates_and_forward_dependencies() {
    let mut writer = CheckpointWriter::new(MODEL);
    assert!(
        writer
            .chunk(
                ChunkFamily::Field,
                1,
                0,
                Vec::new(),
                &[ChunkFamily::Configuration]
            )
            .is_err(),
        "a dependency not written before it"
    );
    writer
        .chunk(ChunkFamily::Configuration, 1, 0, Vec::new(), &[])
        .expect("first");
    assert!(
        writer
            .chunk(ChunkFamily::Configuration, 1, 0, Vec::new(), &[])
            .is_err(),
        "a family written twice"
    );
    assert!(
        writer
            .chunk(
                ChunkFamily::Field,
                1,
                0,
                Vec::new(),
                &[ChunkFamily::Configuration; 17]
            )
            .is_err(),
        "too many dependencies"
    );
    assert!(
        CheckpointWriter::new(&"m".repeat(300)).finish().is_err(),
        "a long model"
    );
}

/// A change to a checkpoint's bytes.
type Edit = Box<dyn Fn(&mut Vec<u8>)>;

#[test]
fn the_reader_refuses_each_container_defect_by_name() {
    let good = sample();
    let defect = |edit: &dyn Fn(&mut Vec<u8>), seal: bool| {
        let mut bytes = good.clone();
        edit(&mut bytes);
        if seal {
            reseal(&mut bytes);
        }
        refusal(&bytes)
    };
    let cases: [(&str, Edit, bool); 13] = [
        ("magic", Box::new(|b| b[0] = b'X'), false),
        ("container format 2", Box::new(|b| b[8] = 2), false),
        ("hash algorithm 7", Box::new(|b| b[12] = 7), false),
        (
            "model identifier of",
            Box::new(|b| b[13..15].copy_from_slice(&300_u16.to_le_bytes())),
            false,
        ),
        (
            "manifest of",
            Box::new(|b| {
                let (start, _) = manifest(b);
                b[start - 4..start].copy_from_slice(&(MANIFEST_MAX_BYTES + 1).to_le_bytes());
            }),
            false,
        ),
        (
            "header or manifest hash mismatch",
            Box::new(|b| b[20] ^= 1),
            false,
        ),
        (
            "unknown chunk family 200",
            Box::new(|b| {
                let at = entry(b, 1);
                b[at] = 200;
            }),
            true,
        ),
        (
            "unsupported codec 1",
            Box::new(|b| {
                let at = entry(b, 1);
                b[at + 1] = 1;
            }),
            true,
        ),
        (
            "encoded and",
            Box::new(|b| {
                let at = entry(b, 1) + 28;
                b[at] ^= 1;
            }),
            true,
        ),
        (
            "overlap or leave a gap",
            Box::new(|b| {
                let at = entry(b, 1) + 12;
                b[at] += 1;
            }),
            true,
        ),
        (
            "duplicate chunk configuration",
            Box::new(|b| {
                let at = entry(b, 1);
                b[at] = ChunkFamily::Configuration.code();
            }),
            true,
        ),
        (
            "does not precede it",
            Box::new(|b| {
                let at = entry(b, 1) + 1 + 1 + 2 + 8 + 8 + 8 + 8 + 32 + 1;
                b[at] = ChunkFamily::Ledger.code();
            }),
            true,
        ),
        (
            "chunk ledger hash mismatch",
            Box::new(|b| {
                let last = b.len() - 1;
                b[last] ^= 1;
            }),
            false,
        ),
    ];
    for (expected, edit, seal) in &cases {
        let reason = defect(edit.as_ref(), *seal);
        assert!(reason.contains(expected), "{expected}: {reason}");
    }
    let mut longer = good.clone();
    longer.push(0);
    assert!(refusal(&longer).contains("payload bytes"));
    for length in 0..good.len() {
        refusal(&good[..length]);
    }
}

#[test]
fn byte_readers_bound_counts_before_allocation_and_refuse_leftovers() {
    let mut out = ByteWriter::new();
    out.u64(1 << 40);
    out.u8(2);
    out.u32(7);
    let bytes = out.into_bytes();
    let mut input = ByteReader::new(&bytes, "test chunk");
    assert!(input.count(usize::MAX, 8).is_err(), "more items than bytes");
    let mut input = ByteReader::new(&bytes, "test chunk");
    assert!(input.count(10, 1).is_err(), "more items than allowed");
    let mut input = ByteReader::new(&bytes[8..], "test chunk");
    assert!(input.flag().is_err(), "a flag byte of 2");
    let input = ByteReader::new(&bytes, "test chunk");
    assert!(input.finish().is_err(), "unread bytes");
    let mut input = ByteReader::new(&bytes[9..], "test chunk");
    assert_eq!(input.u32().expect("four bytes"), 7);
    input.finish().expect("all read");
}
