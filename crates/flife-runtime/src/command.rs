//! The command protocol's types (plan §12.3).
//!
//! A [`Command`] is what a client submits: its session, its own command
//! identity, the boundary it targets, the configuration revision it expects
//! and its validated payload. Admission answers with an [`Admission`]: the
//! accepted order and effective tick, or a [`Rejection`]. Every accepted
//! command ends in exactly one terminal [`Outcome`]: applied, cancelled
//! before application, or failed.

use flife_core::canonical::Sha256Digest;
use flife_core::checkpoint::ByteReader;
use flife_core::ids::{CommandId, ResourceEpoch, SessionId};
use flife_core::{CoreError, Tick};
use flife_engine::Intervention;

/// The canonical command record's schema.
pub const COMMAND_SCHEMA: &str = "frankenlife.command.v1";

/// `volatile-in-memory`: acceptance and outcomes are held by the session
/// owner and acknowledged as volatile. Everything since the session opened
/// is lost if the process ends (plan §12.3's labelled best-effort mode).
pub const VOLATILE_IN_MEMORY: &str = "volatile-in-memory";

/// The boundary a command asks to apply at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The boundary of committed tick `tick`: the command changes the
    /// candidate that advances `tick` to `tick + 1`. A boundary that is
    /// already sealed or published is refused, unless `retarget` permits
    /// the next unsealed boundary instead.
    At {
        /// The committed tick whose boundary it applies at.
        tick: Tick,
        /// Whether a late command may move to the next unsealed boundary.
        retarget: bool,
    },
    /// The next boundary not yet sealed.
    NextAdmissible,
}

/// A simulation-changing command as a client submits it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Command {
    /// The session it is addressed to.
    pub session: SessionId,
    /// The client's identity for it, unique within the session.
    pub id: CommandId,
    /// Where it applies.
    pub target: Target,
    /// The configuration hash the client expects the session to run, if it
    /// states one.
    pub expected_revision: Option<Sha256Digest>,
    /// What it does.
    pub intervention: Intervention,
}

impl Command {
    /// The canonical record of everything the client stated: schema,
    /// session, identity, target, expected revision and payload, with every
    /// number as its little-endian bits.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(128);
        bytes.extend_from_slice(COMMAND_SCHEMA.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&self.session.get().to_le_bytes());
        bytes.extend_from_slice(&self.id.get().to_le_bytes());
        match self.target {
            Target::At { tick, retarget } => {
                bytes.push(1);
                bytes.extend_from_slice(&tick.get().to_le_bytes());
                bytes.push(u8::from(retarget));
            }
            Target::NextAdmissible => bytes.push(2),
        }
        match self.expected_revision {
            Some(revision) => {
                bytes.push(1);
                bytes.extend_from_slice(revision.as_bytes());
            }
            None => bytes.push(0),
        }
        match self.intervention {
            Intervention::InjectEnergy { cell, amount } => {
                bytes.extend_from_slice(b"inject-energy");
                bytes.push(0);
                bytes.extend_from_slice(&cell[0].to_le_bytes());
                bytes.extend_from_slice(&cell[1].to_le_bytes());
                bytes.extend_from_slice(&amount.to_bits().to_le_bytes());
            }
        }
        bytes
    }

    /// The command whose [`Command::canonical_bytes`] are `bytes`.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] for bytes that are not exactly
    /// one canonical command record.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, CoreError> {
        let refuse = |reason: &str| CoreError::IncompatibleCheckpoint {
            reason: format!("command record: {reason}"),
        };
        let mut input = ByteReader::new(bytes, "command record");
        if input.bytes(COMMAND_SCHEMA.len())? != COMMAND_SCHEMA.as_bytes() || input.u8()? != 0 {
            return Err(refuse("not a frankenlife.command.v1 record"));
        }
        let session = SessionId::new(input.u64()?);
        let id = CommandId::new(input.u64()?);
        let target = match input.u8()? {
            1 => Target::At {
                tick: Tick::new(input.u64()?),
                retarget: input.flag()?,
            },
            2 => Target::NextAdmissible,
            _ => return Err(refuse("unknown target")),
        };
        let expected_revision = if input.flag()? {
            Some(Sha256Digest::from_bytes(input.array()?))
        } else {
            None
        };
        let kind = b"inject-energy";
        if input.bytes(kind.len())? != kind || input.u8()? != 0 {
            return Err(refuse("unknown payload"));
        }
        let intervention = Intervention::InjectEnergy {
            cell: [input.u32()?, input.u32()?],
            amount: input.f64()?,
        };
        input.finish()?;
        let command = Self {
            session,
            id,
            target,
            expected_revision,
            intervention,
        };
        if command.canonical_bytes() != bytes {
            return Err(refuse("not in canonical form"));
        }
        Ok(command)
    }

    /// The SHA-256 of [`Command::canonical_bytes`]. A retry with the same
    /// identity must carry the same content hash.
    #[must_use]
    pub fn content_hash(&self) -> Sha256Digest {
        Sha256Digest::of(&self.canonical_bytes())
    }
}

/// How an acceptance is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// `volatile-in-memory`: lost if the process ends.
    Volatile,
    /// `write-ahead-before-apply`: the command journal (FL-036) holds the
    /// record. A client is told of the acceptance only once the journal's
    /// receipt says the record and every one admitted before it are
    /// durable, and no candidate applies the command before then.
    WriteAhead,
}

/// What admission assigned an accepted command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accepted {
    /// Its position in the session's admission order, from zero.
    pub order: u64,
    /// The committed tick whose boundary it applies at.
    pub effective: Tick,
    /// Whether a late explicit target was moved to `effective`.
    pub retargeted: bool,
    /// How the acceptance is held.
    pub durability: Durability,
}

/// Why a cancellation ended a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    /// The client cancelled it.
    Requested,
    /// The session closed before it applied.
    SessionClosed,
}

/// An accepted command's terminal result.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Applied at the boundary of `tick` and published as `published`.
    Applied {
        /// The committed tick whose boundary it applied at.
        tick: Tick,
        /// The tick that publication committed.
        published: Tick,
        /// The epoch that publication committed.
        epoch: ResourceEpoch,
        /// The energy that entered the field.
        applied: f64,
        /// Its index in the session's outcome stream.
        cursor: u64,
    },
    /// Cancelled before it applied, with the committed tick then.
    Cancelled {
        /// The committed tick when the cancellation won.
        boundary: Tick,
        /// Who cancelled it.
        reason: CancelReason,
        /// Its index in the session's outcome stream.
        cursor: u64,
    },
    /// It can never apply.
    Failed {
        /// Why.
        detail: String,
        /// Its index in the session's outcome stream.
        cursor: u64,
    },
}

impl Outcome {
    /// Its index in the session's outcome stream.
    #[must_use]
    pub const fn cursor(&self) -> u64 {
        match self {
            Self::Applied { cursor, .. }
            | Self::Cancelled { cursor, .. }
            | Self::Failed { cursor, .. } => *cursor,
        }
    }
}

/// Where an accepted command is.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandStatus {
    /// Accepted and waiting for its boundary.
    Pending,
    /// Sealed into the candidate for its boundary. A candidate that pauses
    /// or is discarded leaves it sealed for the identical retry.
    Sealed,
    /// Finished.
    Terminal(Outcome),
}

/// Why admission refused a command. Nothing is recorded for it: a retry
/// with the same identity is admitted afresh.
#[derive(Debug, Clone, PartialEq)]
pub enum Rejection {
    /// Addressed to another session.
    WrongSession {
        /// This session.
        session: SessionId,
    },
    /// The client expected another configuration.
    RevisionMismatch {
        /// The configuration this session runs.
        revision: Sha256Digest,
    },
    /// The payload failed validation.
    Invalid(CoreError),
    /// Its explicit target is already sealed or published, and it did not
    /// permit retargeting.
    Late {
        /// The boundary it asked for.
        requested: Tick,
        /// The earliest boundary still open.
        earliest: Tick,
    },
    /// The scientific queue holds its capacity of unfinished commands.
    QueueFull {
        /// The queue's capacity.
        capacity: usize,
    },
    /// Its identity was accepted earlier with different content.
    ConflictingReuse {
        /// The earlier content's hash.
        accepted: Sha256Digest,
    },
    /// The run stopped under `stop-and-preserve`; nothing more can apply.
    Stopped,
    /// The session is closing or closed.
    Closed,
}

/// Admission's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Admission {
    /// Accepted now, or earlier with the same content (`retry`).
    Accepted {
        /// What admission assigned it.
        accepted: Accepted,
        /// Where it is now.
        status: CommandStatus,
        /// Whether this repeats an earlier acceptance.
        retry: bool,
    },
    /// Refused; nothing was recorded.
    Rejected(Rejection),
}

/// The answer to a cancellation request.
#[derive(Debug, Clone, PartialEq)]
pub enum Cancellation {
    /// The cancellation won: the command will never apply. `sealed` says
    /// whether a candidate already held it and was discarded.
    Cancelled {
        /// The terminal outcome recorded.
        outcome: Outcome,
        /// Whether it had been sealed into a candidate.
        sealed: bool,
    },
    /// Publication won: the command already applied.
    TooLate(Outcome),
    /// It had already ended otherwise.
    AlreadyTerminal(Outcome),
    /// No command with this identity was accepted.
    Unknown,
}
