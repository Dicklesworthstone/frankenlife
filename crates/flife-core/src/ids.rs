//! Stable identities and epoch identifiers (plan §4.1, §9.5, §16.2).
//!
//! An identity is never a storage slot. Each sequence issues strictly
//! increasing values and never reissues one, so reusing a slot never reuses
//! an identity, its randomness or its held controller state. Exhausting a
//! sequence is a refused [`CoreError::Overflow`], never a wrap.

use std::fmt;
use std::marker::PhantomData;

use crate::error::CoreError;

/// An identity issued by an [`IdSequence`].
pub trait SequentialId: Copy + Ord {
    /// The quantity named in overflow errors.
    const QUANTITY: &'static str;
    /// Wrap a raw value.
    fn from_raw(raw: u64) -> Self;
    /// The canonical unsigned integer (plan A.3 item 10).
    fn raw(self) -> u64;
}

macro_rules! sequential_id {
    ($(#[$doc:meta])* $name:ident, $label:literal, $quantity:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            /// Wrap a raw value, for example one read from a checkpoint.
            #[must_use]
            pub const fn new(raw: u64) -> Self {
                Self(raw)
            }

            /// The canonical unsigned integer.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl SequentialId for $name {
            const QUANTITY: &'static str = $quantity;
            fn from_raw(raw: u64) -> Self {
                Self(raw)
            }
            fn raw(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($label, ":{}"), self.0)
            }
        }
    };
}

sequential_id!(
    /// A live or dead organism's stable identity. It addresses the
    /// organism's randomness and fixes its controller phase.
    OrganismId,
    "organism",
    "organism id"
);
sequential_id!(
    /// A founder lineage. Every descendant carries its founder's lineage.
    LineageId,
    "lineage",
    "lineage id"
);
sequential_id!(
    /// An immutable genome in the genome store.
    GenomeId,
    "genome",
    "genome id"
);
sequential_id!(
    /// A client command, unique within its session.
    CommandId,
    "command",
    "command id"
);
sequential_id!(
    /// A runtime session.
    SessionId,
    "session",
    "session id"
);
sequential_id!(
    /// An immutable render epoch: one committed world published to readers.
    RenderEpoch,
    "render-epoch",
    "render epoch"
);
sequential_id!(
    /// A resource epoch that a typed lease names (plan §9.5).
    ResourceEpoch,
    "resource-epoch",
    "resource epoch"
);

/// A fixed environment source. Sources are numbered `0..count` at
/// initialization and never created or destroyed in the baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceId(u32);

impl SourceId {
    /// Wrap a source ordinal.
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// The source ordinal.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "source:{}", self.0)
    }
}

/// Generations since the founder (founders are generation 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(u32);

impl Generation {
    /// A founder's generation.
    pub const FOUNDER: Self = Self(0);

    /// Wrap a raw generation.
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// The generation count.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// A child's generation.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] when the parent is at `u32::MAX`.
    pub fn child(self) -> Result<Self, CoreError> {
        self.0.checked_add(1).map(Self).ok_or(CoreError::Overflow {
            quantity: "generation",
        })
    }
}

/// Issues identities in strictly increasing order and never reissues one.
///
/// The sequence state (`next`) is checkpointed so that a resumed run
/// continues the same sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdSequence<T> {
    next: Option<u64>,
    issued: PhantomData<fn() -> T>,
}

impl<T: SequentialId> IdSequence<T> {
    /// A sequence whose first identity is `first`.
    #[must_use]
    pub const fn starting_at(first: u64) -> Self {
        Self {
            next: Some(first),
            issued: PhantomData,
        }
    }

    /// Resume from checkpointed state; `None` means exhausted.
    #[must_use]
    pub const fn resume(next: Option<u64>) -> Self {
        Self {
            next,
            issued: PhantomData,
        }
    }

    /// The raw value the next [`issue`](Self::issue) returns, for
    /// checkpointing; `None` once every value has been issued.
    #[must_use]
    pub const fn next_raw(&self) -> Option<u64> {
        self.next
    }

    /// Issue the next identity.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] once `u64::MAX` has been issued.
    pub fn issue(&mut self) -> Result<T, CoreError> {
        let raw = self.next.ok_or(CoreError::Overflow {
            quantity: T::QUANTITY,
        })?;
        self.next = raw.checked_add(1);
        Ok(T::from_raw(raw))
    }
}
