//! The session owner's command book (plan §12.3): idempotent admission,
//! the pending schedule, sealed boundaries, cancellation and the outcome
//! stream. It is a deterministic state machine. The owner calls it in the
//! order messages and publications occur, so a test can drive it directly.
//!
//! - Admission assigns each accepted command one position in the admission
//!   order and one effective boundary. The pending schedule is ordered by
//!   `(effective tick, admission order)`, so admission order and application
//!   order can differ: A accepted for tick 1000 before B for tick 101 stays
//!   pending when B applies.
//! - [`CommandBook::seal`] fixes the exact ordered set for the committed
//!   tick's boundary before the candidate is computed. A sealed boundary
//!   admits nothing new: an explicit target there is late. A candidate
//!   that pauses or is discarded leaves its boundary sealed, so the retry
//!   applies the identical set, less any command whose cancellation won.
//! - [`CommandBook::published`] records the sealed set applied, in order,
//!   from the published tick's report.
//! - The outcome stream numbers terminal outcomes in the order they occur.
//!   Its cursor is not the admission order.

use std::collections::BTreeMap;

use flife_core::canonical::Sha256Digest;
use flife_core::ids::{CommandId, SessionId};
use flife_core::{CoreError, Tick};
use flife_engine::{Intervention, TickReport};
use flife_field::FieldGrid;

use crate::command::{
    Accepted, Admission, CancelReason, Cancellation, Command, CommandStatus, Durability, Outcome,
    Rejection, Target,
};

#[derive(Debug, Clone)]
struct Record {
    content: Sha256Digest,
    command: Command,
    accepted: Accepted,
    status: CommandStatus,
}

/// A session's accepted commands and their outcomes.
#[derive(Debug, Clone)]
pub struct CommandBook {
    session: SessionId,
    revision: Sha256Digest,
    capacity: usize,
    records: BTreeMap<CommandId, Record>,
    pending: BTreeMap<(Tick, u64), CommandId>,
    sealed: Option<Tick>,
    next_order: u64,
    outcomes: Vec<CommandId>,
    closed: bool,
}

impl CommandBook {
    /// An empty book for `session` running configuration `revision`, holding
    /// at most `capacity` unfinished commands.
    #[must_use]
    pub const fn new(session: SessionId, revision: Sha256Digest, capacity: usize) -> Self {
        Self {
            session,
            revision,
            capacity,
            records: BTreeMap::new(),
            pending: BTreeMap::new(),
            sealed: None,
            next_order: 0,
            outcomes: Vec::new(),
            closed: false,
        }
    }

    /// The session.
    #[must_use]
    pub const fn session(&self) -> SessionId {
        self.session
    }

    /// The configuration hash the session runs.
    #[must_use]
    pub const fn revision(&self) -> Sha256Digest {
        self.revision
    }

    /// The most unfinished commands the book holds.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Unfinished commands: pending or sealed.
    #[must_use]
    pub fn unfinished(&self) -> usize {
        self.pending.len()
    }

    /// The next admission order to be assigned: the count of commands ever
    /// accepted.
    #[must_use]
    pub const fn admitted(&self) -> u64 {
        self.next_order
    }

    /// The outcome stream's cursor: the count of terminal outcomes.
    #[must_use]
    pub fn outcome_cursor(&self) -> u64 {
        u64::try_from(self.outcomes.len()).unwrap_or(u64::MAX)
    }

    /// The commands in the outcome stream, in the order they ended.
    #[must_use]
    pub fn outcome_stream(&self) -> &[CommandId] {
        &self.outcomes
    }

    /// The boundary sealed into the current or last discarded candidate.
    #[must_use]
    pub const fn sealed(&self) -> Option<Tick> {
        self.sealed
    }

    /// The earliest boundary that still admits commands when the world is
    /// committed at `committed`.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] past the last tick.
    pub fn earliest(&self, committed: Tick) -> Result<Tick, CoreError> {
        if self.sealed == Some(committed) {
            committed.next()
        } else {
            Ok(committed)
        }
    }

    /// The pending schedule, in application order: `(effective tick,
    /// admission order, command)`.
    pub fn schedule(&self) -> impl Iterator<Item = (Tick, u64, CommandId)> + '_ {
        self.pending
            .iter()
            .map(|(&(tick, order), &id)| (tick, order, id))
    }

    /// A command's status.
    #[must_use]
    pub fn status(&self, id: CommandId) -> Option<&CommandStatus> {
        self.records.get(&id).map(|record| &record.status)
    }

    /// A command's acceptance.
    #[must_use]
    pub fn accepted(&self, id: CommandId) -> Option<Accepted> {
        self.records.get(&id).map(|record| record.accepted)
    }

    /// Admit `command` while the world is committed at `committed` on
    /// `grid`. A repeat of an accepted identity with the same content
    /// returns that acceptance and its current status; with different
    /// content it is refused. `stopped` refuses new work after a numerical
    /// fault stopped the run.
    pub fn admit(
        &mut self,
        command: Command,
        committed: Tick,
        grid: &FieldGrid,
        stopped: bool,
    ) -> Admission {
        if command.session != self.session {
            return Admission::Rejected(Rejection::WrongSession {
                session: self.session,
            });
        }
        let content = command.content_hash();
        if let Some(record) = self.records.get(&command.id) {
            if record.content != content {
                return Admission::Rejected(Rejection::ConflictingReuse {
                    accepted: record.content,
                });
            }
            return Admission::Accepted {
                accepted: record.accepted,
                status: record.status.clone(),
                retry: true,
            };
        }
        if self.closed {
            return Admission::Rejected(Rejection::Closed);
        }
        if stopped {
            return Admission::Rejected(Rejection::Stopped);
        }
        if command
            .expected_revision
            .is_some_and(|revision| revision != self.revision)
        {
            return Admission::Rejected(Rejection::RevisionMismatch {
                revision: self.revision,
            });
        }
        if let Err(error) = command.intervention.check(grid) {
            return Admission::Rejected(Rejection::Invalid(error));
        }
        let earliest = match self.earliest(committed) {
            Ok(tick) => tick,
            Err(error) => return Admission::Rejected(Rejection::Invalid(error)),
        };
        let (effective, retargeted) = match command.target {
            Target::NextAdmissible => (earliest, false),
            Target::At { tick, .. } if tick >= earliest => (tick, false),
            Target::At { retarget: true, .. } => (earliest, true),
            Target::At { tick, .. } => {
                return Admission::Rejected(Rejection::Late {
                    requested: tick,
                    earliest,
                });
            }
        };
        if self.pending.len() >= self.capacity {
            return Admission::Rejected(Rejection::QueueFull {
                capacity: self.capacity,
            });
        }
        let accepted = Accepted {
            order: self.next_order,
            effective,
            retargeted,
            durability: Durability::Volatile,
        };
        self.next_order += 1;
        self.pending.insert((effective, accepted.order), command.id);
        self.records.insert(
            command.id,
            Record {
                content,
                command,
                accepted,
                status: CommandStatus::Pending,
            },
        );
        Admission::Accepted {
            accepted,
            status: CommandStatus::Pending,
            retry: false,
        }
    }

    /// Seal the boundary of `committed`: its commands in admission order,
    /// with their interventions. Sealing it again, after a discarded or
    /// paused candidate, returns the same set less any cancelled since.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when another boundary is sealed, or a
    /// pending command's boundary has already passed.
    pub fn seal(&mut self, committed: Tick) -> Result<Vec<(CommandId, Intervention)>, CoreError> {
        if let Some(sealed) = self.sealed
            && sealed != committed
        {
            return Err(CoreError::StateConflict {
                operation: "seal command boundary",
                detail: format!(
                    "tick {} is sealed, not tick {}",
                    sealed.get(),
                    committed.get()
                ),
            });
        }
        if let Some((&(tick, _), id)) = self.pending.first_key_value()
            && tick < committed
        {
            return Err(CoreError::StateConflict {
                operation: "seal command boundary",
                detail: format!(
                    "{id} was due at tick {} but the world is at tick {}",
                    tick.get(),
                    committed.get()
                ),
            });
        }
        self.sealed = Some(committed);
        let mut set = Vec::new();
        for (_, &id) in self.pending.range((committed, 0)..=(committed, u64::MAX)) {
            if let Some(record) = self.records.get_mut(&id) {
                record.status = CommandStatus::Sealed;
                set.push((id, record.command.intervention));
            }
        }
        Ok(set)
    }

    /// Record publication of `report`, the candidate built from `sealed`,
    /// the set [`CommandBook::seal`] returned for its input tick.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when the report is not of the sealed
    /// boundary or does not apply exactly the sealed set in order. Nothing
    /// is recorded then.
    pub fn published(
        &mut self,
        report: &TickReport,
        sealed: &[(CommandId, Intervention)],
    ) -> Result<(), CoreError> {
        let conflict = |detail: String| CoreError::StateConflict {
            operation: "record published commands",
            detail,
        };
        if self.sealed != Some(report.from) {
            return Err(conflict(format!(
                "tick {} was published but {:?} is sealed",
                report.from.get(),
                self.sealed.map(Tick::get)
            )));
        }
        let current: Vec<CommandId> = self
            .pending
            .range((report.from, 0)..=(report.from, u64::MAX))
            .map(|(_, &id)| id)
            .collect();
        let offered: Vec<CommandId> = sealed.iter().map(|&(id, _)| id).collect();
        if current != offered
            || report.interventions.len() != sealed.len()
            || report.interventions.iter().zip(sealed).enumerate().any(
                |(position, (applied, (_, intervention)))| {
                    applied.position != position
                        || applied.tick != report.from
                        || applied.intervention != *intervention
                },
            )
        {
            return Err(conflict(format!(
                "the report applies {} interventions; the sealed set is {offered:?}, \
                 now {current:?}",
                report.interventions.len()
            )));
        }
        for ((id, _), applied) in sealed.iter().zip(&report.interventions) {
            let cursor = self.outcome_cursor();
            if let Some(record) = self.records.get_mut(id) {
                self.pending.remove(&(report.from, record.accepted.order));
                record.status = CommandStatus::Terminal(Outcome::Applied {
                    tick: report.from,
                    published: report.tick,
                    epoch: report.epoch,
                    applied: applied.applied,
                    cursor,
                });
                self.outcomes.push(*id);
            }
        }
        self.sealed = None;
        Ok(())
    }

    /// Cancel `id` while the world is committed at `committed`. A pending or
    /// sealed command is cancelled and will never apply; the owner must
    /// discard a candidate that holds a sealed one before publishing it.
    pub fn cancel(&mut self, id: CommandId, committed: Tick) -> Cancellation {
        self.end(id, committed, CancelReason::Requested)
    }

    fn end(&mut self, id: CommandId, committed: Tick, reason: CancelReason) -> Cancellation {
        let cursor = self.outcome_cursor();
        let Some(record) = self.records.get_mut(&id) else {
            return Cancellation::Unknown;
        };
        let sealed = match &record.status {
            CommandStatus::Terminal(outcome @ Outcome::Applied { .. }) => {
                return Cancellation::TooLate(outcome.clone());
            }
            CommandStatus::Terminal(outcome) => {
                return Cancellation::AlreadyTerminal(outcome.clone());
            }
            CommandStatus::Pending => false,
            CommandStatus::Sealed => true,
        };
        let outcome = Outcome::Cancelled {
            boundary: committed,
            reason,
            cursor,
        };
        self.pending
            .remove(&(record.accepted.effective, record.accepted.order));
        record.status = CommandStatus::Terminal(outcome.clone());
        self.outcomes.push(id);
        Cancellation::Cancelled { outcome, sealed }
    }

    /// End every unfinished command as failed with `detail`: the run
    /// stopped, so none can apply. Returns how many ended.
    pub fn fail_unfinished(&mut self, detail: &str) -> usize {
        let unfinished: Vec<CommandId> = self.pending.values().copied().collect();
        for id in &unfinished {
            let cursor = self.outcome_cursor();
            if let Some(record) = self.records.get_mut(id) {
                record.status = CommandStatus::Terminal(Outcome::Failed {
                    detail: detail.to_owned(),
                    cursor,
                });
                self.outcomes.push(*id);
            }
        }
        self.pending.clear();
        self.sealed = None;
        unfinished.len()
    }

    /// Close the book while the world is committed at `committed`: every
    /// unfinished command is cancelled as `session-closed`, in schedule
    /// order, and admission refuses new identities. Returns how many ended.
    pub fn close(&mut self, committed: Tick) -> usize {
        let unfinished: Vec<CommandId> = self.pending.values().copied().collect();
        for &id in &unfinished {
            let _ = self.end(id, committed, CancelReason::SessionClosed);
        }
        self.sealed = None;
        self.closed = true;
        unfinished.len()
    }

    /// Whether the book is closed.
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        self.closed
    }
}
