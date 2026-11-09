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
//!   cancellation and the outcome stream.
//! - [`checkpoint`]: a committed world and its complete command closure as
//!   one verified checkpoint, and their exact restoration (FL-016).
//! - [`session`]: the Asupersync owner task. It holds the committed world
//!   and the book, serves a reserved control lane and a bounded scientific
//!   lane, stays responsive while paused, advances one candidate at a time
//!   with a cancellation boundary before publication, and is the only commit
//!   authority.
//!
//! Acceptance here is `volatile-in-memory`: held by the owner and lost if
//! the process ends. Appendix A's `write-ahead-before-apply` needs FL-036's
//! command journal and is refused until then.

#![forbid(unsafe_code)]

pub mod book;
pub mod checkpoint;
pub mod command;
pub mod session;

pub use book::{COMMANDS_CHUNK_VERSION, CommandBook};
pub use checkpoint::{Restored, SESSION_CHECKPOINT_CHUNKS, checkpoint, restore};
pub use command::{
    Accepted, Admission, COMMAND_SCHEMA, CancelReason, Cancellation, Command, CommandStatus,
    Durability, Outcome, Rejection, Target, VOLATILE_IN_MEMORY,
};
pub use session::{
    Boundary, RunEnd, Running, SessionClosed, SessionConfig, SessionHandle, SessionOwner, Snapshot,
    StepOutcome, WRITE_AHEAD_BEFORE_APPLY, open, open_with,
};
