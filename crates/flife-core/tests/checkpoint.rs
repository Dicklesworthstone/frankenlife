//! FL-016's checkpoint container (Appendix B.2): what the writer emits
//! parses back to the same chunks, and each manifest defect is refused by
//! name even when the header and manifest are re-sealed around it.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::io::Read;

use flife_core::CoreError;
use flife_core::Sha256Digest;
use flife_core::binary16::F16;
use flife_core::checkpoint::{
    ByteReader, ByteWriter, CHECKPOINT_FORMAT, CHECKPOINT_PAGE_BYTES, CheckpointManifest,
    CheckpointReader, CheckpointWriter, ChunkFamily, ChunkSink, ChunkStream, ChunkWriter,
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

/// Records in [`records`]' field chunk, and each one's length: more than
/// two pages in all, so the stream's window is refilled and compacted.
const RECORDS: usize = 40;
const RECORD_BYTES: usize = 64 << 10;

/// A container whose field chunk is a count, then [`RECORDS`] records of a
/// length and that many bytes, between a configuration and a ledger chunk.
fn records() -> Vec<u8> {
    let mut field = ByteWriter::new();
    field.u64(RECORDS as u64);
    for record in 0..RECORDS {
        field.u64(RECORD_BYTES as u64);
        for at in 0..RECORD_BYTES {
            field.u8(u8::try_from((record * 31 + at) % 251).expect("a byte"));
        }
    }
    let mut writer = CheckpointWriter::new(MODEL);
    writer
        .chunk(ChunkFamily::Configuration, 1, 1, b"config".to_vec(), &[])
        .expect("first");
    writer
        .chunk(
            ChunkFamily::Field,
            3,
            RECORDS as u64,
            field.into_bytes(),
            &[],
        )
        .expect("second");
    writer
        .chunk(ChunkFamily::Ledger, 1, 1, b"ledger".to_vec(), &[])
        .expect("third");
    writer.finish().expect("finishes")
}

/// `bytes` without `family`'s chunk, and that chunk's bytes.
fn split(bytes: &[u8], family: ChunkFamily) -> (Vec<u8>, Vec<u8>) {
    let manifest = CheckpointManifest::parse(bytes).expect("a manifest");
    let payload = &bytes[manifest.sealed_length()..];
    let mut held = bytes[..manifest.sealed_length()].to_vec();
    let mut streamed = Vec::new();
    for entry in manifest.entries() {
        let start = usize::try_from(entry.offset).expect("small");
        let end = start + usize::try_from(entry.length).expect("small");
        if entry.family == family {
            streamed.extend_from_slice(&payload[start..end]);
        } else {
            held.extend_from_slice(&payload[start..end]);
        }
    }
    (held, streamed)
}

/// A source that gives at most 1,000 bytes a read, is interrupted every
/// other read, and fails after `fails_after` bytes if that is set.
struct Trickle {
    bytes: Vec<u8>,
    at: usize,
    reads: usize,
    fails_after: Option<usize>,
}

impl Trickle {
    fn new(bytes: Vec<u8>) -> Box<Self> {
        Box::new(Self {
            bytes,
            at: 0,
            reads: 0,
            fails_after: None,
        })
    }
}

impl Read for Trickle {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        self.reads += 1;
        if self.reads.is_multiple_of(2) {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        if self.fails_after.is_some_and(|after| self.at >= after) {
            return Err(std::io::Error::other("the device went away"));
        }
        let length = out.len().min(1_000).min(self.bytes.len() - self.at);
        out[..length].copy_from_slice(&self.bytes[self.at..self.at + length]);
        self.at += length;
        Ok(length)
    }
}

/// Decode [`records`]' field chunk from `stream`, a record at a time, and
/// check every byte; then finish the stream. The largest window held is
/// returned.
fn drain(mut stream: ChunkStream<'_>, check: bool) -> Result<u64, CoreError> {
    let count = stream.count(RECORDS, 8)?;
    assert_eq!(count, RECORDS);
    let mut largest = stream.held_bytes();
    for record in 0..count {
        let length = usize::try_from(stream.u64()?).expect("small");
        stream.fill(length)?;
        assert!(
            stream.window().len() >= length,
            "the window holds the record"
        );
        if check {
            for (at, byte) in stream.window()[..length].iter().enumerate() {
                assert_eq!(usize::from(*byte), (record * 31 + at) % 251);
            }
        }
        stream.advance(length);
        largest = largest.max(stream.held_bytes());
    }
    assert_eq!(stream.items(), RECORDS as u64);
    assert_eq!(stream.remaining(), 0);
    stream.finish()?;
    Ok(largest)
}

fn incompatible(result: Result<impl std::fmt::Debug, CoreError>) -> String {
    match result {
        Err(CoreError::IncompatibleCheckpoint { reason }) => reason,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_streamed_chunk_reads_as_the_held_one_from_one_window() {
    let whole = records();
    let (held, streamed) = split(&whole, ChunkFamily::Field);
    assert_eq!(held.len() + streamed.len(), whole.len());
    let reader = CheckpointReader::parse_streamed(
        &held,
        &[ChunkFamily::Configuration, ChunkFamily::Field],
        ChunkFamily::Field,
        Some(Trickle::new(streamed.clone())),
    )
    .expect("parses without its field chunk");
    assert_eq!(reader.model(), MODEL);
    assert_eq!(reader.held_bytes(), held.len() as u64);
    assert_eq!(
        reader.families().collect::<Vec<_>>(),
        [
            ChunkFamily::Configuration,
            ChunkFamily::Field,
            ChunkFamily::Ledger
        ]
    );
    assert_eq!(
        reader
            .versioned(ChunkFamily::Ledger, 1)
            .expect("a held chunk")
            .bytes,
        b"ledger"
    );
    assert!(
        incompatible(reader.chunk(ChunkFamily::Field)).contains("is streamed, not held"),
        "the streamed chunk is not held"
    );
    assert!(
        incompatible(reader.stream(ChunkFamily::Field, 1)).contains("version 3"),
        "another version"
    );
    let stream = reader.stream(ChunkFamily::Field, 3).expect("streams");
    assert_eq!((stream.version(), stream.items()), (3, RECORDS as u64));
    let window = drain(stream, true).expect("decodes and verifies");
    assert_eq!(
        window, CHECKPOINT_PAGE_BYTES as u64,
        "one page-sized window, reused for every record"
    );
    assert!(window * 2 < streamed.len() as u64);
    assert!(
        incompatible(reader.stream(ChunkFamily::Field, 3)).contains("streamed already"),
        "the source is read once"
    );

    let reader = CheckpointReader::parse(&whole, &[]).expect("parses whole");
    assert_eq!(reader.held_bytes(), whole.len() as u64);
    let stream = reader.stream(ChunkFamily::Field, 3).expect("a held stream");
    assert_eq!(drain(stream, true).expect("decodes"), 0, "it holds nothing");
    let ledger = reader.stream(ChunkFamily::Ledger, 1).expect("again");
    assert_eq!(ledger.window(), b"ledger");
    assert!(
        incompatible(ledger.finish()).contains("6 unread bytes"),
        "a held stream left unread"
    );
}

#[test]
fn a_streamed_chunk_is_verified_at_its_end_and_refused_by_name() {
    let whole = records();
    let (held, streamed) = split(&whole, ChunkFamily::Field);
    let parse = |source: Box<dyn Read>| {
        CheckpointReader::parse_streamed(&held, &[], ChunkFamily::Field, Some(source))
            .expect("parses")
    };

    let mut tampered = streamed.clone();
    let middle = tampered.len() / 2;
    tampered[middle] ^= 1;
    let reader = parse(Trickle::new(tampered));
    let stream = reader.stream(ChunkFamily::Field, 3).expect("streams");
    assert!(
        incompatible(drain(stream, false)).contains("chunk field hash mismatch"),
        "a changed byte is found at the end"
    );

    let reader = parse(Trickle::new(streamed[..streamed.len() - 1].to_vec()));
    let stream = reader.stream(ChunkFamily::Field, 3).expect("streams");
    let reason = incompatible(drain(stream, false));
    assert!(
        reason.contains("chunk field ends after"),
        "a short source: {reason}"
    );

    let mut failing = Trickle::new(streamed.clone());
    failing.fails_after = Some(streamed.len() / 3);
    let reader = parse(failing);
    let stream = reader.stream(ChunkFamily::Field, 3).expect("streams");
    match drain(stream, false) {
        Err(CoreError::StorageFailed {
            operation, reason, ..
        }) => assert_eq!(
            (operation, reason.as_str()),
            ("read chunk", "the device went away")
        ),
        other => panic!("{other:?}"),
    }

    let mut longer = streamed.clone();
    longer.extend_from_slice(&[0; 8]);
    let reader = parse(Trickle::new(longer.clone()));
    let stream = reader.stream(ChunkFamily::Field, 3).expect("streams");
    assert_eq!(
        drain(stream, true).expect("reads the chunk and no further"),
        CHECKPOINT_PAGE_BYTES as u64,
        "a source longer than its chunk"
    );

    let mut declared = ByteWriter::new();
    declared.u64(u64::MAX);
    longer[..8].copy_from_slice(&declared.into_bytes());
    let reader = parse(Trickle::new(longer));
    let mut stream = reader.stream(ChunkFamily::Field, 3).expect("streams");
    assert!(
        incompatible(stream.count(usize::MAX, 8)).contains("bytes remain"),
        "a count the chunk cannot hold"
    );

    assert!(
        incompatible(CheckpointReader::parse_streamed(
            &held,
            &[],
            ChunkFamily::Field,
            None
        ))
        .contains("no source"),
        "a streamed chunk without its source"
    );
    let mut trailing = held.clone();
    trailing.push(0);
    assert!(
        incompatible(CheckpointReader::parse_streamed(
            &trailing,
            &[],
            ChunkFamily::Field,
            Some(Trickle::new(streamed.clone()))
        ))
        .contains("held payload bytes"),
        "a held payload longer than its held chunks"
    );
    let mut bad_ledger = held.clone();
    let last = bad_ledger.len() - 1;
    bad_ledger[last] ^= 1;
    assert!(
        incompatible(CheckpointReader::parse_streamed(
            &bad_ledger,
            &[],
            ChunkFamily::Field,
            Some(Trickle::new(streamed.clone()))
        ))
        .contains("chunk ledger hash mismatch"),
        "a held chunk is verified as parse verifies it"
    );
    assert!(
        incompatible(CheckpointReader::parse_streamed(
            &held,
            &[ChunkFamily::Genomes],
            ChunkFamily::Field,
            Some(Trickle::new(streamed))
        ))
        .contains("missing chunk genomes"),
        "a required family"
    );
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

#[derive(Default)]
struct RecordingSink {
    pages: Vec<Vec<u8>>,
    length: u64,
    digest: Option<Sha256Digest>,
    fail_on_page: Option<usize>,
}

impl ChunkSink for RecordingSink {
    fn begin(&mut self, _index: usize, _family: ChunkFamily) -> Result<(), CoreError> {
        self.pages.clear();
        self.length = 0;
        self.digest = None;
        Ok(())
    }

    fn page(&mut self, bytes: &[u8]) -> Result<(), CoreError> {
        if self.fail_on_page == Some(self.pages.len()) {
            return Err(CoreError::StorageFailed {
                operation: "write page",
                path: "recording-sink".into(),
                reason: "injected page fault".into(),
            });
        }
        assert!(
            bytes.len() <= CHECKPOINT_PAGE_BYTES,
            "page length {} exceeded CHECKPOINT_PAGE_BYTES ({CHECKPOINT_PAGE_BYTES})",
            bytes.len()
        );
        self.pages.push(bytes.to_vec());
        Ok(())
    }

    fn end(&mut self, length: u64, digest: Sha256Digest) -> Result<(), CoreError> {
        self.length = length;
        self.digest = Some(digest);
        Ok(())
    }
}

fn stream_chunk(
    encode: impl FnOnce(&mut ChunkWriter<'_>) -> Result<(), CoreError>,
) -> (Vec<u8>, Vec<usize>, Sha256Digest) {
    let mut sink = RecordingSink::default();
    let mut writer = CheckpointWriter::streaming(MODEL, &mut sink);
    writer
        .chunk_with(ChunkFamily::Genomes, 1, 1, &[], encode)
        .expect("streams chunk");
    let _sealed = writer.seal().expect("seals streamed chunk");
    let page_sizes: Vec<usize> = sink.pages.iter().map(Vec::len).collect();
    if let Some((_, intermediate)) = page_sizes.split_last() {
        for &size in intermediate {
            assert_eq!(
                size, CHECKPOINT_PAGE_BYTES,
                "every intermediate spilled page must be exactly CHECKPOINT_PAGE_BYTES"
            );
        }
    }
    let bytes: Vec<u8> = sink.pages.into_iter().flatten().collect();
    assert_eq!(
        u64::try_from(bytes.len()).expect("fits u64"),
        sink.length,
        "streamed length matches concatenated pages"
    );
    let digest = sink.digest.expect("sink recorded digest");
    assert_eq!(digest, Sha256Digest::of(&bytes));
    (bytes, page_sizes, digest)
}

fn memory_chunk(
    encode: impl FnOnce(&mut ChunkWriter<'_>) -> Result<(), CoreError>,
) -> (Vec<u8>, Sha256Digest) {
    let mut writer = CheckpointWriter::new(MODEL);
    writer
        .chunk_with(ChunkFamily::Genomes, 1, 1, &[], encode)
        .expect("encodes in-memory chunk");
    let container = writer.finish().expect("finishes in-memory container");
    let reader = CheckpointReader::parse(&container, &[ChunkFamily::Genomes]).expect("parses");
    let chunk = reader.chunk(ChunkFamily::Genomes).expect("chunk");
    let bytes = chunk.bytes.to_vec();
    let digest = Sha256Digest::of(&bytes);
    (bytes, digest)
}

fn make_f32_values(count: usize) -> Vec<f32> {
    (0..count)
        .map(|i| match i % 7 {
            0 => 0.0_f32,
            1 => -0.0_f32,
            2 => f32::from_bits(1_u32.wrapping_add((i as u32) & 0x3f)),
            3 => f32::from_bits(0x3f80_0000 ^ ((i as u32).wrapping_mul(0x9e37_79b9) & 0x007f_ffff)),
            4 => -((i as f32) * 0.125 + 1.0),
            5 => f32::from_bits(0x7fc0_0001),
            _ => (i as f32) * 0.0625,
        })
        .collect()
}

fn make_u16_values(count: usize) -> Vec<u16> {
    (0..count)
        .map(|i| (i as u16).wrapping_mul(251).wrapping_add(17))
        .collect()
}

fn make_f16_values(count: usize) -> Vec<F16> {
    make_u16_values(count)
        .into_iter()
        .map(F16::from_bits)
        .collect()
}

#[test]
fn bulk_slice_writers_match_element_wise_across_page_straddles() {
    let page = CHECKPOINT_PAGE_BYTES;
    let f32_per_page = page / 4;
    let u16_per_page = page / 2;

    // 1. Exact element counts hitting empty, tiny, page-1, page, page+1, and 3*page+13 byte sizes.
    let f32_counts = [
        0,
        1,
        7,
        f32_per_page - 1,
        f32_per_page,
        f32_per_page + 1,
        3 * f32_per_page + 13,
    ];
    let f32_max = make_f32_values(*f32_counts.iter().max().expect("nonempty"));
    for count in f32_counts {
        let slice = &f32_max[..count];
        let (want_bytes, want_pages, want_digest) = stream_chunk(|out| {
            for &v in slice {
                out.f32(v);
            }
            Ok(())
        });
        let (got_bytes, got_pages, got_digest) = stream_chunk(|out| {
            out.f32_slice(slice);
            Ok(())
        });
        assert_eq!(got_digest, want_digest, "f32_slice digest count={count}");
        assert_eq!(got_pages, want_pages, "f32_slice pages count={count}");
        assert_eq!(got_bytes, want_bytes, "f32_slice bytes count={count}");
        let (mem_bytes, mem_digest) = memory_chunk(|out| {
            out.f32_slice(slice);
            Ok(())
        });
        assert_eq!(
            mem_digest, want_digest,
            "f32_slice mem digest count={count}"
        );
        assert_eq!(mem_bytes, want_bytes, "f32_slice mem bytes count={count}");
    }

    let u16_counts = [
        0,
        1,
        7,
        u16_per_page - 1,
        u16_per_page,
        u16_per_page + 1,
        3 * u16_per_page + 13,
    ];
    let u16_max = make_u16_values(*u16_counts.iter().max().expect("nonempty"));
    let f16_max = make_f16_values(*u16_counts.iter().max().expect("nonempty"));
    for count in u16_counts {
        let u_slice = &u16_max[..count];
        let (want_bytes, want_pages, want_digest) = stream_chunk(|out| {
            for &v in u_slice {
                out.u16(v);
            }
            Ok(())
        });
        let (got_bytes, got_pages, got_digest) = stream_chunk(|out| {
            out.u16_slice(u_slice);
            Ok(())
        });
        assert_eq!(got_digest, want_digest, "u16_slice digest count={count}");
        assert_eq!(got_pages, want_pages, "u16_slice pages count={count}");
        assert_eq!(got_bytes, want_bytes, "u16_slice bytes count={count}");

        let f_slice = &f16_max[..count];
        let (got_f16_bytes, got_f16_pages, got_f16_digest) = stream_chunk(|out| {
            out.f16_slice(f_slice);
            Ok(())
        });
        assert_eq!(
            got_f16_digest, want_digest,
            "f16_slice digest count={count}"
        );
        assert_eq!(got_f16_pages, want_pages, "f16_slice pages count={count}");
        assert_eq!(got_f16_bytes, want_bytes, "f16_slice bytes count={count}");

        let (mem_f16_bytes, mem_f16_digest) = memory_chunk(|out| {
            out.f16_slice(f_slice);
            Ok(())
        });
        assert_eq!(
            mem_f16_digest, want_digest,
            "f16_slice mem digest count={count}"
        );
        assert_eq!(
            mem_f16_bytes, want_bytes,
            "f16_slice mem bytes count={count}"
        );
    }

    // 2. Unaligned prefix lengths straddling page boundaries (0, 1, 7, page-3, page-2, page-1, page, page+1).
    let straddle_prefixes = [0, 1, 7, page - 3, page - 2, page - 1, page, page + 1];
    let f32_span = &f32_max[..f32_per_page + 13];
    let f16_span = &f16_max[..u16_per_page + 13];
    for prefix_len in straddle_prefixes {
        let prefix = vec![0x5a_u8; prefix_len];
        let (want_f32, want_f32_pages, want_f32_digest) = stream_chunk(|out| {
            out.bytes(&prefix);
            for &v in f32_span {
                out.f32(v);
            }
            Ok(())
        });
        let (got_f32, got_f32_pages, got_f32_digest) = stream_chunk(|out| {
            out.bytes(&prefix);
            out.f32_slice(f32_span);
            Ok(())
        });
        assert_eq!(
            got_f32_digest, want_f32_digest,
            "f32 straddle prefix={prefix_len}"
        );
        assert_eq!(
            got_f32_pages, want_f32_pages,
            "f32 straddle pages prefix={prefix_len}"
        );
        assert_eq!(got_f32, want_f32, "f32 straddle bytes prefix={prefix_len}");

        let (want_f16, want_f16_pages, want_f16_digest) = stream_chunk(|out| {
            out.bytes(&prefix);
            for &v in f16_span {
                out.u16(v.to_bits());
            }
            Ok(())
        });
        let (got_f16, got_f16_pages, got_f16_digest) = stream_chunk(|out| {
            out.bytes(&prefix);
            out.f16_slice(f16_span);
            Ok(())
        });
        assert_eq!(
            got_f16_digest, want_f16_digest,
            "f16 straddle prefix={prefix_len}"
        );
        assert_eq!(
            got_f16_pages, want_f16_pages,
            "f16 straddle pages prefix={prefix_len}"
        );
        assert_eq!(got_f16, want_f16, "f16 straddle bytes prefix={prefix_len}");
    }

    // 3. Sink failure mid-slice stops cleanly and returns the sink's error.
    let mut failing = RecordingSink {
        fail_on_page: Some(1),
        ..RecordingSink::default()
    };
    let mut writer = CheckpointWriter::streaming(MODEL, &mut failing);
    let err = writer
        .chunk_with(ChunkFamily::Genomes, 1, 1, &[], |out| {
            out.f16_slice(&f16_max[..3 * u16_per_page + 13]);
            Ok(())
        })
        .expect_err("mid-slice page fault must propagate");
    assert!(
        matches!(
            err,
            CoreError::StorageFailed {
                operation: "write page",
                ..
            }
        ),
        "unexpected error: {err:?}"
    );
}
