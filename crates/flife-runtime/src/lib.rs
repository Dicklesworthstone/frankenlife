//! FrankenLife's world-session owner and command admission (FL-014; plan
//! §12.1–§12.4, §8.1 phase 1, Appendix C.8 and C.14).
//!
//! - [`command`]: the protocol's types. A client [`Command`] carries its
//!   session, its client command identity, its target boundary, the
//!   configuration revision it expects and its validated payload. Admission
//!   answers with the accepted order and effective tick, or a rejection.
//!   Every accepted command ends applied, cancelled before application, or
//!   failed.
//! - [`book`]: the owner's [`CommandBook`], a deterministic state machine:
//!   idempotent admission by `(session, command id)`, the pending schedule
//!   ordered by `(effective tick, admission order)`, sealed boundaries,
//!   cancellation and the outcome stream; a [`Fork`] into a branch
//!   session (FL-039).
//! - [`checkpoint`](mod@checkpoint): a committed world and its complete command closure as
//!   one verified checkpoint, and their exact restoration (FL-016).
//! - [`session`]: the Asupersync owner task. It holds the committed world
//!   and the book, serves a reserved control lane and a bounded scientific
//!   lane, stays responsive while paused, advances one candidate at a time
//!   with a cancellation boundary before publication, and is the only commit
//!   authority.
//!
//! Acceptance is `volatile-in-memory` (held by the owner and lost if the
//! process ends) or, through [`journal`]'s port and [`open_journaled`],
//! Appendix A's `write-ahead-before-apply` (FL-036): made durable before it
//! is acknowledged or applied, and recovered from the journal.

#![forbid(unsafe_code)]

pub mod book;
pub mod checkpoint;
pub mod command;
pub mod journal;
pub mod session;

pub use book::{
    BRANCHED_COMMANDS_CHUNK_VERSION, COMMANDS_CHUNK_VERSION, CommandBook, Fork, admitting_session,
};
pub use checkpoint::{Restored, SESSION_CHECKPOINT_CHUNKS, checkpoint, checkpoint_into, restore};
pub use command::{
    Accepted, Admission, COMMAND_SCHEMA, CancelReason, Cancellation, Command, CommandStatus,
    Durability, Outcome, Rejection, Target, VOLATILE_IN_MEMORY,
};
pub use journal::{
    JOURNAL_ATTEMPTS, Journal, JournalContents, JournalRecord, JournalWriter, Watermarks,
};
pub use session::{
    Boundary, JournalState, RunEnd, Running, SessionCheckpoint, SessionClosed, SessionConfig,
    SessionHandle, SessionOwner, Snapshot, StepOutcome, WRITE_AHEAD_BEFORE_APPLY, open,
    open_journaled, open_restored, open_with,
};
