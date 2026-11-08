//! Distinct failure classes (plan §18.3).
//!
//! An API that can fail says how. Missing capability, budget exhaustion,
//! schema mismatch, an incompatible checkpoint, queue closure, numerical
//! invalidity, cancellation, integer overflow, a buffer layout mismatch, a
//! refused allocation and a state conflict are separate variants, so a
//! caller can never mistake one for another.

use std::fmt;

/// One configuration problem, located by its `section.key` path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    /// The offending key, as `section.key` (or `toml` for a parse failure).
    pub path: String,
    /// What is wrong with it.
    pub problem: String,
}

impl fmt::Display for ConfigIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.problem)
    }
}

/// A FrankenLife core failure.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreError {
    /// A required host, device or upstream capability is absent.
    MissingCapability {
        /// The capability that is missing.
        capability: String,
    },
    /// A declared budget cannot cover a request.
    BudgetExhausted {
        /// The budget, for example `logical genome bytes`.
        budget: &'static str,
        /// What the request needs.
        required: u64,
        /// What the budget has left.
        available: u64,
    },
    /// Data does not match the schema or version this build expects.
    SchemaMismatch {
        /// The schema this build accepts.
        expected: String,
        /// The schema that was found.
        found: String,
    },
    /// A checkpoint cannot be resumed by this model or build.
    IncompatibleCheckpoint {
        /// Why it is incompatible.
        reason: String,
    },
    /// A queue or channel was closed.
    QueueClosed {
        /// The queue that closed.
        queue: &'static str,
    },
    /// A nonfinite, negative or otherwise invalid number.
    NumericalInvalid {
        /// The quantity being computed or validated.
        quantity: &'static str,
        /// The offending value and why it is refused.
        detail: String,
    },
    /// Work was cancelled before it completed.
    Cancelled {
        /// Why it was cancelled.
        reason: String,
    },
    /// A checked integer (tick, identity, deadline or byte count) left its
    /// representable range. Refused before anything is published.
    Overflow {
        /// The quantity that would overflow.
        quantity: &'static str,
    },
    /// A named algorithm is not in the registry, or not admitted for the
    /// role it was used in.
    UnknownAlgorithm {
        /// The configuration key that named it.
        role: &'static str,
        /// The name that was given.
        name: String,
    },
    /// A configuration failed validation. Every issue found is listed.
    InvalidConfig(Vec<ConfigIssue>),
    /// A buffer's declared layout is impossible, or differs from the layout
    /// its reader requires (plan §9.3). The buffer is refused, never
    /// reinterpreted from its length.
    LayoutMismatch {
        /// The buffer, for example `field`.
        buffer: &'static str,
        /// The first difference or impossibility found.
        detail: String,
    },
    /// The host refused an allocation that every checked limit admitted. It
    /// is reported instead of aborting the process (INV-15).
    AllocationFailed {
        /// The buffer being allocated.
        buffer: &'static str,
        /// The bytes requested.
        bytes: u64,
    },
    /// Work prepared against one state was offered to a different state, or
    /// offered twice: for example uptake grants applied to a field that has
    /// changed since they were resolved. Nothing is changed.
    StateConflict {
        /// The operation that was refused.
        operation: &'static str,
        /// The conflict that was found.
        detail: String,
    },
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCapability { capability } => {
                write!(f, "missing capability: {capability}")
            }
            Self::BudgetExhausted {
                budget,
                required,
                available,
            } => write!(
                f,
                "{budget} budget exhausted: {required} required, {available} available"
            ),
            Self::SchemaMismatch { expected, found } => {
                write!(f, "schema mismatch: expected {expected}, found {found}")
            }
            Self::IncompatibleCheckpoint { reason } => {
                write!(f, "incompatible checkpoint: {reason}")
            }
            Self::QueueClosed { queue } => write!(f, "queue closed: {queue}"),
            Self::NumericalInvalid { quantity, detail } => {
                write!(f, "invalid {quantity}: {detail}")
            }
            Self::Cancelled { reason } => write!(f, "cancelled: {reason}"),
            Self::Overflow { quantity } => write!(f, "{quantity} overflow refused"),
            Self::UnknownAlgorithm { role, name } => {
                write!(f, "{role} names unknown or inadmissible algorithm `{name}`")
            }
            Self::LayoutMismatch { buffer, detail } => {
                write!(f, "{buffer} layout refused: {detail}")
            }
            Self::AllocationFailed { buffer, bytes } => {
                write!(f, "host refused a {bytes}-byte {buffer} allocation")
            }
            Self::StateConflict { operation, detail } => {
                write!(f, "{operation} refused: {detail}")
            }
            Self::InvalidConfig(issues) => {
                write!(f, "invalid configuration ({} issues)", issues.len())?;
                for issue in issues {
                    write!(f, "; {issue}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for CoreError {}
