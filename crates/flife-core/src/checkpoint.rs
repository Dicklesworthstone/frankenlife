//! The checkpoint container (plan §16.3, Appendix B.2; FL-016).
//!
//! A checkpoint is one byte string in canonical little-endian order:
//!
//! | Part | Content |
//! |---|---|
//! | header | [`CHECKPOINT_MAGIC`], format [`CHECKPOINT_FORMAT`] (`u32`), hash algorithm [`HASH_SHA256`] (`u8`), the model identifier (`u16` length, UTF-8), the manifest's length (`u32`) |
//! | manifest | the chunk count (`u16`); per chunk its family, codec, schema version, logical item count, payload offset, encoded and decoded sizes, SHA-256, and the families it depends on |
//! | seal | the SHA-256 of the header and manifest |
//! | payload | the chunks, back to back in manifest order |
//!
//! [`CheckpointReader::parse`] refuses, before allocating anything, a wrong
//! magic, format, hash algorithm or seal; a manifest or chunk count beyond
//! its bound; an unknown family or codec; a duplicate or missing family; a
//! dependency on a family not written before it (so references cannot
//! form a cycle); chunks that do not tile the payload exactly (overlap, a
//! gap or trailing bytes); and a chunk whose hash differs. Chunk decoders
//! read through [`ByteReader`], which checks every declared count against
//! the bytes that remain before reserving storage and refuses unread bytes.
//!
//! [`CheckpointManifest::parse`] verifies the sealed prefix alone, so a chunk
//! store can keep it as the checkpoint's manifest and the chunks by digest.
//!
//! Every chunk is stored raw ([`CODEC_RAW`]): its encoded and decoded sizes
//! are equal and its hash is also its content identity, so no codec can
//! change a genome's meaning.

use crate::canonical::Sha256Digest;
use crate::error::CoreError;

/// The first eight bytes of every checkpoint.
pub const CHECKPOINT_MAGIC: [u8; 8] = *b"FLIFECKP";

/// The container format this build writes and reads.
pub const CHECKPOINT_FORMAT: u32 = 1;

/// The hash algorithm code for SHA-256, the only one this build uses.
pub const HASH_SHA256: u8 = 1;

/// The codec code for raw bytes, the only one this build uses.
pub const CODEC_RAW: u8 = 0;

/// The largest manifest a checkpoint may declare.
pub const MANIFEST_MAX_BYTES: u32 = 64 * 1024;

/// The longest model identifier a checkpoint may declare.
pub const MODEL_MAX_BYTES: u16 = 256;

/// The most families a chunk may depend on.
pub const DEPENDENCIES_MAX: u8 = 16;

/// What a chunk holds (Appendix B.2's chunk families).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChunkFamily {
    /// The full effective configuration, its hash, the slot arrangement
    /// and the numerical profile.
    Configuration,
    /// The random addressing algorithm and seed. There are no stateful
    /// streams: every draw is addressed (§16.2).
    Random,
    /// The topology catalog the genomes index.
    Topology,
    /// The committed tick and resource epoch, and each source's path
    /// segment.
    Environment,
    /// Identity allocators, the event count and the genome quota.
    Lifecycle,
    /// The committed field: energy and moment planes.
    Field,
    /// Every live genome: topology, parent, traits and parameters.
    Genomes,
    /// Every live organism's identity, ancestry and kinematic state.
    Particles,
    /// Held controller state: raw outputs, validity, evaluation tick,
    /// phase, recent energy change, cooldown deadline and birth tick.
    Held,
    /// The energy ledger's holdings and cumulative totals.
    Ledger,
    /// The command closure: every accepted command, pending or terminal,
    /// and the outcome stream.
    Commands,
}

impl ChunkFamily {
    /// Every family, in code order.
    pub const ALL: [Self; 11] = [
        Self::Configuration,
        Self::Random,
        Self::Topology,
        Self::Environment,
        Self::Lifecycle,
        Self::Field,
        Self::Genomes,
        Self::Particles,
        Self::Held,
        Self::Ledger,
        Self::Commands,
    ];

    /// Its code in a manifest.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Configuration => 1,
            Self::Random => 2,
            Self::Topology => 3,
            Self::Environment => 4,
            Self::Lifecycle => 5,
            Self::Field => 6,
            Self::Genomes => 7,
            Self::Particles => 8,
            Self::Held => 9,
            Self::Ledger => 10,
            Self::Commands => 11,
        }
    }

    /// The family with `code`, if any.
    #[must_use]
    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|family| family.code() == code)
    }

    /// Its name, for refusals.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::Random => "random",
            Self::Topology => "topology",
            Self::Environment => "environment",
            Self::Lifecycle => "lifecycle",
            Self::Field => "field",
            Self::Genomes => "genomes",
            Self::Particles => "particles",
            Self::Held => "held",
            Self::Ledger => "ledger",
            Self::Commands => "commands",
        }
    }
}

fn refuse(reason: impl Into<String>) -> CoreError {
    CoreError::IncompatibleCheckpoint {
        reason: reason.into(),
    }
}

/// One chunk as written: its schema version, its logical item count and
/// its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk<'a> {
    /// The chunk's own schema version.
    pub version: u16,
    /// How many logical items it holds (organisms, genomes, values, …).
    pub items: u64,
    /// Its bytes.
    pub bytes: &'a [u8],
}

#[derive(Debug, Clone)]
struct Written {
    family: ChunkFamily,
    version: u16,
    items: u64,
    bytes: Vec<u8>,
    depends: Vec<ChunkFamily>,
}

/// Builds a checkpoint chunk by chunk.
#[derive(Debug, Clone)]
pub struct CheckpointWriter {
    model: String,
    chunks: Vec<Written>,
}

impl CheckpointWriter {
    /// An empty checkpoint of model `model`.
    #[must_use]
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_owned(),
            chunks: Vec::new(),
        }
    }

    /// The families written so far, in order.
    pub fn families(&self) -> impl Iterator<Item = ChunkFamily> + '_ {
        self.chunks.iter().map(|chunk| chunk.family)
    }

    /// Add `family`'s chunk, which reads the families in `depends`.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] for a family already written,
    /// a dependency not written before it, or more than
    /// [`DEPENDENCIES_MAX`] dependencies.
    pub fn chunk(
        &mut self,
        family: ChunkFamily,
        version: u16,
        items: u64,
        bytes: Vec<u8>,
        depends: &[ChunkFamily],
    ) -> Result<(), CoreError> {
        if self.chunks.iter().any(|chunk| chunk.family == family) {
            return Err(refuse(format!("chunk {} written twice", family.name())));
        }
        if depends.len() > usize::from(DEPENDENCIES_MAX) {
            return Err(refuse(format!(
                "chunk {} has {} dependencies",
                family.name(),
                depends.len()
            )));
        }
        if let Some(missing) = depends
            .iter()
            .find(|needed| !self.chunks.iter().any(|chunk| chunk.family == **needed))
        {
            return Err(refuse(format!(
                "chunk {} depends on {}, which is not written before it",
                family.name(),
                missing.name()
            )));
        }
        self.chunks.push(Written {
            family,
            version,
            items,
            bytes,
            depends: depends.to_vec(),
        });
        Ok(())
    }

    /// The checkpoint's bytes.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] when the model identifier, the
    /// manifest or the chunk count exceeds its bound.
    pub fn finish(self) -> Result<Vec<u8>, CoreError> {
        let model = self.model.as_bytes();
        let model_length = u16::try_from(model.len())
            .ok()
            .filter(|length| *length <= MODEL_MAX_BYTES)
            .ok_or_else(|| refuse(format!("model identifier of {} bytes", model.len())))?;
        let count = u16::try_from(self.chunks.len())
            .map_err(|_| refuse(format!("{} chunks", self.chunks.len())))?;

        let mut manifest = ByteWriter::new();
        manifest.u16(count);
        let mut offset = 0_u64;
        for chunk in &self.chunks {
            let length = u64::try_from(chunk.bytes.len()).unwrap_or(u64::MAX);
            manifest.u8(chunk.family.code());
            manifest.u8(CODEC_RAW);
            manifest.u16(chunk.version);
            manifest.u64(chunk.items);
            manifest.u64(offset);
            manifest.u64(length);
            manifest.u64(length);
            manifest.bytes(Sha256Digest::of(&chunk.bytes).as_bytes());
            manifest.u8(u8::try_from(chunk.depends.len()).unwrap_or(u8::MAX));
            for needed in &chunk.depends {
                manifest.u8(needed.code());
            }
            offset = offset.saturating_add(length);
        }
        let manifest = manifest.into_bytes();
        let manifest_length = u32::try_from(manifest.len())
            .ok()
            .filter(|length| *length <= MANIFEST_MAX_BYTES)
            .ok_or_else(|| refuse(format!("manifest of {} bytes", manifest.len())))?;

        let mut out = ByteWriter::new();
        out.bytes(&CHECKPOINT_MAGIC);
        out.u32(CHECKPOINT_FORMAT);
        out.u8(HASH_SHA256);
        out.u16(model_length);
        out.bytes(model);
        out.u32(manifest_length);
        out.bytes(&manifest);
        let seal = Sha256Digest::of(out.as_slice());
        out.bytes(seal.as_bytes());
        for chunk in &self.chunks {
            out.bytes(&chunk.bytes);
        }
        Ok(out.into_bytes())
    }
}

/// One chunk's manifest entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestEntry {
    /// The chunk's family.
    pub family: ChunkFamily,
    /// Its schema version.
    pub version: u16,
    /// Its logical item count.
    pub items: u64,
    /// Where it starts in the payload.
    pub offset: u64,
    /// Its length in bytes (raw: encoded and decoded alike).
    pub length: u64,
    /// The SHA-256 of its bytes, which is also its content identity.
    pub digest: Sha256Digest,
}

/// A checkpoint's sealed prefix (header, manifest and seal), verified
/// without its payload. A chunk store keeps this prefix as the
/// checkpoint's manifest and the chunks by their digests (FL-035).
#[derive(Debug, Clone)]
pub struct CheckpointManifest<'a> {
    model: &'a str,
    entries: Vec<ManifestEntry>,
    sealed: usize,
}

impl<'a> CheckpointManifest<'a> {
    /// Verify the sealed prefix at the start of `bytes`, which may
    /// continue with a payload or end at the seal. Checks everything
    /// [`CheckpointReader::parse`] checks except the chunk hashes and how
    /// the chunks cover a payload.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] naming the first defect.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, CoreError> {
        let mut header = ByteReader::new(bytes, "checkpoint header");
        if header.array::<8>()? != CHECKPOINT_MAGIC {
            return Err(refuse("not a FrankenLife checkpoint (magic)"));
        }
        let format = header.u32()?;
        if format != CHECKPOINT_FORMAT {
            return Err(refuse(format!(
                "container format {format}; this build reads {CHECKPOINT_FORMAT}"
            )));
        }
        let hash = header.u8()?;
        if hash != HASH_SHA256 {
            return Err(refuse(format!(
                "hash algorithm {hash}; this build reads SHA-256"
            )));
        }
        let model_length = header.u16()?;
        if model_length > MODEL_MAX_BYTES {
            return Err(refuse(format!("model identifier of {model_length} bytes")));
        }
        let model = std::str::from_utf8(header.bytes(usize::from(model_length))?)
            .map_err(|_| refuse("model identifier is not UTF-8"))?;
        let manifest_length = header.u32()?;
        if manifest_length > MANIFEST_MAX_BYTES {
            return Err(refuse(format!("manifest of {manifest_length} bytes")));
        }
        let manifest_bytes =
            header.bytes(usize::try_from(manifest_length).unwrap_or(usize::MAX))?;
        let sealed = header.at;
        let seal = header.array::<32>()?;
        if Sha256Digest::of(&bytes[..sealed]).as_bytes() != &seal {
            return Err(refuse("header or manifest hash mismatch"));
        }

        let mut manifest = ByteReader::new(manifest_bytes, "checkpoint manifest");
        let count = manifest.u16()?;
        let mut entries: Vec<ManifestEntry> = Vec::new();
        let mut expected = 0_u64;
        for _ in 0..count {
            let code = manifest.u8()?;
            let family = ChunkFamily::from_code(code)
                .ok_or_else(|| refuse(format!("unknown chunk family {code}")))?;
            if entries.iter().any(|entry| entry.family == family) {
                return Err(refuse(format!("duplicate chunk {}", family.name())));
            }
            let codec = manifest.u8()?;
            if codec != CODEC_RAW {
                return Err(refuse(format!(
                    "chunk {} uses unsupported codec {codec}",
                    family.name()
                )));
            }
            let version = manifest.u16()?;
            let items = manifest.u64()?;
            let offset = manifest.u64()?;
            let encoded = manifest.u64()?;
            let decoded = manifest.u64()?;
            let digest = Sha256Digest::from_bytes(manifest.array::<32>()?);
            let dependencies = manifest.u8()?;
            if dependencies > DEPENDENCIES_MAX {
                return Err(refuse(format!(
                    "chunk {} declares {dependencies} dependencies",
                    family.name()
                )));
            }
            for _ in 0..dependencies {
                let code = manifest.u8()?;
                let needed = ChunkFamily::from_code(code).ok_or_else(|| {
                    refuse(format!(
                        "chunk {} depends on unknown family {code}",
                        family.name()
                    ))
                })?;
                if needed == family || !entries.iter().any(|entry| entry.family == needed) {
                    return Err(refuse(format!(
                        "chunk {} depends on {}, which does not precede it",
                        family.name(),
                        needed.name()
                    )));
                }
            }
            if encoded != decoded {
                return Err(refuse(format!(
                    "raw chunk {} declares {encoded} encoded and {decoded} decoded bytes",
                    family.name()
                )));
            }
            if offset != expected {
                return Err(refuse(format!(
                    "chunk {} starts at {offset}, not {expected}: chunks overlap or leave a gap",
                    family.name()
                )));
            }
            expected = offset.checked_add(encoded).ok_or_else(|| {
                refuse(format!(
                    "chunk {} declares {encoded} bytes at {offset}",
                    family.name()
                ))
            })?;
            entries.push(ManifestEntry {
                family,
                version,
                items,
                offset,
                length: encoded,
                digest,
            });
        }
        manifest.finish()?;
        Ok(Self {
            model,
            entries,
            sealed: sealed + 32,
        })
    }

    /// The model identifier.
    #[must_use]
    pub const fn model(&self) -> &'a str {
        self.model
    }

    /// The chunks, in manifest (and payload) order.
    #[must_use]
    pub fn entries(&self) -> &[ManifestEntry] {
        &self.entries
    }

    /// The length of the sealed prefix: header, manifest and seal.
    #[must_use]
    pub const fn sealed_length(&self) -> usize {
        self.sealed
    }

    /// The payload length the chunks declare.
    #[must_use]
    pub fn payload_length(&self) -> u64 {
        self.entries
            .last()
            .map_or(0, |entry| entry.offset.saturating_add(entry.length))
    }
}

#[derive(Debug, Clone)]
struct Entry {
    family: ChunkFamily,
    version: u16,
    items: u64,
    start: usize,
    end: usize,
}

/// A parsed checkpoint whose structure and hashes were verified.
#[derive(Debug, Clone)]
pub struct CheckpointReader<'a> {
    model: &'a str,
    entries: Vec<Entry>,
    payload: &'a [u8],
}

impl<'a> CheckpointReader<'a> {
    /// Verify `bytes` (see the [module documentation](self)) and require
    /// every family in `required`.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] naming the first defect.
    pub fn parse(bytes: &'a [u8], required: &[ChunkFamily]) -> Result<Self, CoreError> {
        let manifest = CheckpointManifest::parse(bytes)?;
        let payload = &bytes[manifest.sealed..];
        let available = u64::try_from(payload.len()).unwrap_or(u64::MAX);
        let mut entries = Vec::with_capacity(manifest.entries.len());
        for entry in &manifest.entries {
            let end = entry
                .offset
                .checked_add(entry.length)
                .filter(|end| *end <= available)
                .ok_or_else(|| {
                    refuse(format!(
                        "chunk {} runs past the payload's {} bytes",
                        entry.family.name(),
                        payload.len()
                    ))
                })?;
            let (start, end) = (
                usize::try_from(entry.offset).unwrap_or(usize::MAX),
                usize::try_from(end).unwrap_or(usize::MAX),
            );
            if Sha256Digest::of(&payload[start..end]) != entry.digest {
                return Err(refuse(format!(
                    "chunk {} hash mismatch",
                    entry.family.name()
                )));
            }
            entries.push(Entry {
                family: entry.family,
                version: entry.version,
                items: entry.items,
                start,
                end,
            });
        }
        let expected = manifest.payload_length();
        if expected != available {
            return Err(refuse(format!(
                "{} payload bytes, the chunks cover {expected}",
                payload.len()
            )));
        }
        if let Some(missing) = required
            .iter()
            .find(|family| !entries.iter().any(|entry| entry.family == **family))
        {
            return Err(refuse(format!("missing chunk {}", missing.name())));
        }
        Ok(Self {
            model: manifest.model,
            entries,
            payload,
        })
    }

    /// The model identifier.
    #[must_use]
    pub const fn model(&self) -> &'a str {
        self.model
    }

    /// The families present, in order.
    pub fn families(&self) -> impl Iterator<Item = ChunkFamily> + '_ {
        self.entries.iter().map(|entry| entry.family)
    }

    /// `family`'s chunk.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] if it is absent.
    pub fn chunk(&self, family: ChunkFamily) -> Result<Chunk<'a>, CoreError> {
        self.entries
            .iter()
            .find(|entry| entry.family == family)
            .map(|entry| Chunk {
                version: entry.version,
                items: entry.items,
                bytes: &self.payload[entry.start..entry.end],
            })
            .ok_or_else(|| refuse(format!("missing chunk {}", family.name())))
    }

    /// `family`'s chunk, which must be at schema `version`.
    ///
    /// # Errors
    /// As [`CheckpointReader::chunk`], and for another version.
    pub fn versioned(&self, family: ChunkFamily, version: u16) -> Result<Chunk<'a>, CoreError> {
        let chunk = self.chunk(family)?;
        if chunk.version != version {
            return Err(refuse(format!(
                "chunk {} is version {}; this build reads {version}",
                family.name(),
                chunk.version
            )));
        }
        Ok(chunk)
    }
}

/// Little-endian encoding into a growing buffer.
#[derive(Debug, Clone, Default)]
pub struct ByteWriter(Vec<u8>);

impl ByteWriter {
    /// An empty buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// One byte.
    pub fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    /// A `u16`.
    pub fn u16(&mut self, value: u16) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    /// A `u32`.
    pub fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    /// A `u64`.
    pub fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    /// An `f32`'s bits.
    pub fn f32(&mut self, value: f32) {
        self.u32(value.to_bits());
    }

    /// An `f64`'s bits.
    pub fn f64(&mut self, value: f64) {
        self.u64(value.to_bits());
    }

    /// `None` as `0`, `Some(value)` as `1` then the value.
    pub fn option_u64(&mut self, value: Option<u64>) {
        match value {
            Some(value) => {
                self.u8(1);
                self.u64(value);
            }
            None => self.u8(0),
        }
    }

    /// A length (`u64`), then the bytes.
    pub fn sized(&mut self, bytes: &[u8]) {
        self.u64(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        self.bytes(bytes);
    }

    /// Raw bytes.
    pub fn bytes(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    /// The bytes so far.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// The buffer.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// Bounded little-endian decoding of one chunk or header.
#[derive(Debug, Clone)]
pub struct ByteReader<'a> {
    bytes: &'a [u8],
    at: usize,
    what: &'static str,
}

impl<'a> ByteReader<'a> {
    /// A reader over `bytes`, named `what` in refusals.
    #[must_use]
    pub const fn new(bytes: &'a [u8], what: &'static str) -> Self {
        Self { bytes, at: 0, what }
    }

    /// Bytes not yet read.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    /// The next `length` bytes.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] if fewer remain.
    pub fn bytes(&mut self, length: usize) -> Result<&'a [u8], CoreError> {
        if length > self.remaining() {
            return Err(refuse(format!(
                "{} is truncated: {length} bytes needed at {}, {} remain",
                self.what,
                self.at,
                self.remaining()
            )));
        }
        let bytes = &self.bytes[self.at..self.at + length];
        self.at += length;
        Ok(bytes)
    }

    /// The next `N` bytes.
    ///
    /// # Errors
    /// As [`ByteReader::bytes`].
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], CoreError> {
        let mut out = [0; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    /// One byte.
    ///
    /// # Errors
    /// As [`ByteReader::bytes`].
    pub fn u8(&mut self) -> Result<u8, CoreError> {
        Ok(self.array::<1>()?[0])
    }

    /// A `u16`.
    ///
    /// # Errors
    /// As [`ByteReader::bytes`].
    pub fn u16(&mut self) -> Result<u16, CoreError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    /// A `u32`.
    ///
    /// # Errors
    /// As [`ByteReader::bytes`].
    pub fn u32(&mut self) -> Result<u32, CoreError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    /// A `u64`.
    ///
    /// # Errors
    /// As [`ByteReader::bytes`].
    pub fn u64(&mut self) -> Result<u64, CoreError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    /// An `f32` from its bits.
    ///
    /// # Errors
    /// As [`ByteReader::bytes`].
    pub fn f32(&mut self) -> Result<f32, CoreError> {
        Ok(f32::from_bits(self.u32()?))
    }

    /// An `f64` from its bits.
    ///
    /// # Errors
    /// As [`ByteReader::bytes`].
    pub fn f64(&mut self) -> Result<f64, CoreError> {
        Ok(f64::from_bits(self.u64()?))
    }

    /// A flag written as `0` or `1`.
    ///
    /// # Errors
    /// As [`ByteReader::bytes`], and for any other byte.
    pub fn flag(&mut self) -> Result<bool, CoreError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(refuse(format!("{}: flag byte {other}", self.what))),
        }
    }

    /// An optional `u64` as [`ByteWriter::option_u64`] writes it.
    ///
    /// # Errors
    /// As [`ByteReader::flag`].
    pub fn option_u64(&mut self) -> Result<Option<u64>, CoreError> {
        if self.flag()? {
            Ok(Some(self.u64()?))
        } else {
            Ok(None)
        }
    }

    /// A length-prefixed byte string of at most `maximum` bytes.
    ///
    /// # Errors
    /// As [`ByteReader::bytes`], and for a longer declared length.
    pub fn sized(&mut self, maximum: usize) -> Result<&'a [u8], CoreError> {
        let length = self.count(maximum, 1)?;
        self.bytes(length)
    }

    /// A `u64` count of items of at least `width` bytes each, checked
    /// against `maximum` and against the bytes that remain, so storage for
    /// it can be reserved safely.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] for a count above `maximum` or
    /// one the remaining bytes cannot hold.
    pub fn count(&mut self, maximum: usize, width: usize) -> Result<usize, CoreError> {
        let declared = self.u64()?;
        let count = usize::try_from(declared)
            .ok()
            .filter(|count| *count <= maximum)
            .ok_or_else(|| {
                refuse(format!(
                    "{} declares {declared} items, at most {maximum} allowed",
                    self.what
                ))
            })?;
        if count.saturating_mul(width) > self.remaining() {
            return Err(refuse(format!(
                "{} declares {count} items of {width} bytes, {} bytes remain",
                self.what,
                self.remaining()
            )));
        }
        Ok(count)
    }

    /// Require that every byte was read.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] for unread bytes.
    pub fn finish(self) -> Result<(), CoreError> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(refuse(format!(
                "{} has {} unread bytes",
                self.what,
                self.remaining()
            )))
        }
    }
}
