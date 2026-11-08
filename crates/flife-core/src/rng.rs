//! `fnp-philox-semantic-v1`: semantically addressed random words (plan
//! §16.2, A.3 item 3, INV-11).
//!
//! Every random word in the model is named by what it is for, never by
//! worker, task order, physical slot, wall time or display frame. An
//! [`Address`] is `(stream, entity, ordinal, lane, draw)`, and the run's
//! world seed completes it:
//!
//! - the Philox key is `[world_seed, stream code]`;
//! - the Philox counter is `[entity, ordinal, lane, draw]`;
//! - the address's [`Block`] is the first four raw outputs of NumPy's
//!   `Philox(key=key, counter=counter)`, through FrankenNumPy's NumPy-exact
//!   `PhiloxRng`. NumPy increments the counter before each block, so the
//!   block is Philox4x64-10 of `counter + 1` (mod `2^256`). A device kernel
//!   computes that directly.
//!
//! Distinct addresses have distinct counters, adding one is a bijection on
//! the counter, and Philox is a bijection for a fixed key, so distinct
//! addresses never share a block. A [`Lane`] packs a candidate index, a
//! [`LaneKind`] and a 48-bit index injectively, refusing anything out of
//! range rather than wrapping.
//!
//! Words become draws only through the conventions here: a half-open
//! uniform `k 2^-52` and an open uniform `(k + 1/2) 2^-52` from a word's
//! leading 52 bits `k`, an exact integer choice `⌊k n / 2^52⌋`, and a
//! Box–Muller pair from an open first uniform and a half-open second one,
//! with a fixed component assignment and no cached spare. The logarithm,
//! square root and turn-angle sine and cosine are host-independent
//! ([`crate::numerics`]).

use fnp_random::PhiloxRng;

use crate::numerics::{ln, sin_cos_turns};
use crate::{CoreError, ValidatedConfig};

/// The RNG family this module implements.
pub const FNP_PHILOX_SEMANTIC_V1: &str = "fnp-philox-semantic-v1";

/// `2^-52`.
const UNIT: f64 = 1.0 / 4_503_599_627_370_496.0;

/// Bits of a [`Lane`]'s index.
pub const LANE_INDEX_BITS: u32 = 48;

/// A subsystem's random domain. Its code is part of the Philox key and is
/// frozen in v1: the eight ASCII bytes of its tag, big-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stream {
    /// Founders' genomes and initial positions (A.3 items 1 and 2): entity
    /// is the founder organism, ordinal zero.
    Founders,
    /// Mutation proposals (§7.3, A.3 item 4): entity is the parent
    /// organism, ordinal the attempt tick (the input tick of the advance
    /// that attempts the birth).
    Mutation,
    /// Source chemical directions (A.3 item 7): entity is the source,
    /// ordinal zero.
    SourceChemistry,
    /// Source travel directions (A.3 item 8): entity is the source, ordinal
    /// the path segment.
    SourcePath,
    /// Initial source centers (A.3 item 7): entity is the source, ordinal
    /// zero.
    SourcePlacement,
    /// Reproduction attempts and child offsets (§7.4, A.3 item 10): entity
    /// is the parent organism, ordinal the attempt tick.
    Birth,
}

impl Stream {
    /// Every stream, in code order of declaration.
    pub const ALL: [Self; 6] = [
        Self::Founders,
        Self::Mutation,
        Self::SourceChemistry,
        Self::SourcePath,
        Self::SourcePlacement,
        Self::Birth,
    ];

    /// The tag whose bytes are the code.
    #[must_use]
    pub const fn tag(self) -> &'static [u8; 8] {
        match self {
            Self::Founders => b"founders",
            Self::Mutation => b"mutation",
            Self::SourceChemistry => b"srcchems",
            Self::SourcePath => b"srcpaths",
            Self::SourcePlacement => b"srcplace",
            Self::Birth => b"birthing",
        }
    }

    /// The frozen code in the Philox key.
    #[must_use]
    pub const fn code(self) -> u64 {
        u64::from_be_bytes(*self.tag())
    }
}

/// What a lane's words are for. Codes are frozen in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LaneKind {
    /// A topology-change decision and its target.
    Topology,
    /// One controller weight, named by [`Lane::neuron_index`].
    Weight,
    /// One controller bias, named by [`Lane::neuron_index`] with input zero.
    Bias,
    /// A weight a transition map initializes for an added neuron.
    MapWeight,
    /// One Box–Muller pair of the affinity direction.
    Affinity,
    /// One Box–Muller pair of the motion response.
    MotionResponse,
    /// The sensing scale.
    SensingScale,
    /// A direction: a Box–Muller pair of a source's chemistry, or a travel
    /// angle.
    Direction,
    /// A position: a point on the torus, or a child's offset from its
    /// parent.
    Position,
    /// A Bernoulli attempt, such as a parent's reproduction attempt.
    Attempt,
}

impl LaneKind {
    /// The frozen code.
    #[must_use]
    pub const fn code(self) -> u64 {
        match self {
            Self::Topology => 1,
            Self::Weight => 2,
            Self::Bias => 3,
            Self::MapWeight => 4,
            Self::Affinity => 5,
            Self::MotionResponse => 6,
            Self::SensingScale => 7,
            Self::Direction => 8,
            Self::Position => 9,
            Self::Attempt => 10,
        }
    }
}

/// A packed lane: `candidate << 56 | kind << 48 | index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lane(u64);

impl Lane {
    /// The lane of `kind` and `index` within candidate `candidate`.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] for an index of `2^48` or more.
    pub fn new(candidate: u8, kind: LaneKind, index: u64) -> Result<Self, CoreError> {
        if index >> LANE_INDEX_BITS != 0 {
            return Err(CoreError::Overflow {
                quantity: "random lane index",
            });
        }
        Ok(Self(
            (u64::from(candidate) << 56) | (kind.code() << LANE_INDEX_BITS) | index,
        ))
    }

    /// The index naming one controller parameter by its neuron identity
    /// (A.3 item 5): `layer << 32 | output << 16 | input`.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] for a layer of `2^8` or more, or a neuron
    /// index of `2^16` or more.
    pub fn neuron_index(layer: usize, output: usize, input: usize) -> Result<u64, CoreError> {
        match (
            u8::try_from(layer),
            u16::try_from(output),
            u16::try_from(input),
        ) {
            (Ok(layer), Ok(output), Ok(input)) => {
                Ok((u64::from(layer) << 32) | (u64::from(output) << 16) | u64::from(input))
            }
            _ => Err(CoreError::Overflow {
                quantity: "random lane neuron index",
            }),
        }
    }

    /// The packed word in the Philox counter.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One semantic address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Address {
    /// The subsystem.
    pub stream: Stream,
    /// The stable entity: an organism or source identity.
    pub entity: u64,
    /// The tick, birth ordinal or segment ordinal.
    pub ordinal: u64,
    /// What the words are for.
    pub lane: Lane,
    /// The block within the lane.
    pub draw: u64,
}

/// The four raw words of one address.
pub type Block = [u64; 4];

/// A run's semantic random source: the world seed under
/// `fnp-philox-semantic-v1`. It holds no state; every block is a pure
/// function of the seed and its address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticRng {
    seed: u64,
}

impl SemanticRng {
    /// The source for `seed`.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { seed }
    }

    /// The source of a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] when `mutation.rng` names another
    /// family.
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let model = config.config();
        if model.mutation.rng != FNP_PHILOX_SEMANTIC_V1 {
            return Err(CoreError::UnknownAlgorithm {
                role: "mutation.rng",
                name: model.mutation.rng.clone(),
            });
        }
        Ok(Self::new(model.seed))
    }

    /// The world seed.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// The block at `address`.
    #[must_use]
    pub fn block(&self, address: Address) -> Block {
        let mut philox = PhiloxRng::new(
            [self.seed, address.stream.code()],
            [
                address.entity,
                address.ordinal,
                address.lane.get(),
                address.draw,
            ],
        );
        [
            philox.next_u64(),
            philox.next_u64(),
            philox.next_u64(),
            philox.next_u64(),
        ]
    }
}

/// The half-open uniform `k 2^-52` on `[0, 1)` from a word's leading 52
/// bits `k`.
#[must_use]
pub fn half_open_unit(word: u64) -> f64 {
    // A 52-bit integer converts to F64 exactly, and the scaling is exact.
    (word >> 12) as f64 * UNIT
}

/// The open uniform `(k + 1/2) 2^-52` on `(0, 1)` from a word's leading 52
/// bits `k`. Every step is exact.
#[must_use]
pub fn open_unit(word: u64) -> f64 {
    ((word >> 12) as f64 + 0.5) * UNIT
}

/// A Bernoulli draw: `true` with probability `probability`, as
/// `half_open_unit(word) < probability`. Probability zero is never and one
/// is always.
#[must_use]
pub fn bernoulli(word: u64, probability: f64) -> bool {
    half_open_unit(word) < probability
}

/// A uniform choice in `0..count`, `⌊k count / 2^52⌋` in exact integer
/// arithmetic, or `None` for an empty choice.
#[must_use]
pub fn choose(word: u64, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let count = u128::try_from(count).ok()?;
    usize::try_from((u128::from(word >> 12) * count) >> 52).ok()
}

/// A Box–Muller pair of independent standard normals from two words:
/// `ρ = √(−2 ln u₁)` with the open uniform `u₁` of `first`, the turn
/// `u₂` the half-open uniform of `second`, and `[ρ cos 2πu₂, ρ sin 2πu₂]`
/// in that order. `|ρ| ≤ √(106 ln 2) ≈ 8.57`.
///
/// # Errors
/// None in practice; [`CoreError::NumericalInvalid`] guards the domain.
pub fn box_muller(first: u64, second: u64) -> Result<[f64; 2], CoreError> {
    let invalid = || CoreError::NumericalInvalid {
        quantity: "Box–Muller draw",
        detail: format!("words {first:#018x}, {second:#018x} left the open unit interval"),
    };
    let radius = (-2.0 * ln(open_unit(first)).ok_or_else(invalid)?).sqrt();
    let (sin, cos) = sin_cos_turns(half_open_unit(second)).ok_or_else(invalid)?;
    Ok([radius * cos, radius * sin])
}
