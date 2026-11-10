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
//! - [`CommandBook::branch`] forks a book at a checkpoint into a new
//!   session (FL-039, plan §14.4): the branch inherits every command and
//!   outcome, numbering continues, and the [`Fork`] it records names the
//!   parent session, how many commands that session had admitted, and the
//!   checkpoint. Commands admitted below a fork belong to the session that
//!   admitted them; the branch admits only its own.

use std::collections::{BTreeMap, BTreeSet};

use flife_core::canonical::Sha256Digest;
use flife_core::checkpoint::{
    ByteReader, ByteWriter, CheckpointReader, CheckpointWriter, ChunkFamily,
};
use flife_core::ids::{CommandId, ResourceEpoch, SessionId};
use flife_core::{CoreError, Tick};
use flife_engine::{Intervention, TickReport};
use flife_field::FieldGrid;

use crate::command::{
    Accepted, Admission, CancelReason, Cancellation, Command, CommandStatus, Durability, Outcome,
    Rejection, Target,
};
use crate::journal::JournalContents;

/// Where a session forked from its parent (plan §14.4, §16.3): the parent
/// session, the commands it had admitted, and the checkpoint the branch
/// starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fork {
    /// The session the branch forked from.
    pub parent: SessionId,
    /// Commands the parent had admitted: admission orders below this are
    /// its, or its ancestors'.
    pub admitted: u64,
    /// The committed tick of the checkpoint.
    pub tick: Tick,
    /// The SHA-256 of the checkpoint's container.
    pub checkpoint: Sha256Digest,
}

/// The session that admitted order `order` in a book of `session` with
/// `lineage`, root first.
#[must_use]
pub fn admitting_session(lineage: &[Fork], session: SessionId, order: u64) -> SessionId {
    lineage
        .iter()
        .find(|fork| order < fork.admitted)
        .map_or(session, |fork| fork.parent)
}

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
    lineage: Vec<Fork>,
    revision: Sha256Digest,
    capacity: usize,
    records: BTreeMap<CommandId, Record>,
    pending: BTreeMap<(Tick, u64), CommandId>,
    sealed: Option<Tick>,
    next_order: u64,
    outcomes: Vec<CommandId>,
    closed: bool,
    durability: Durability,
}

impl CommandBook {
    /// An empty book for `session` running configuration `revision`, holding
    /// at most `capacity` unfinished commands.
    #[must_use]
    pub const fn new(session: SessionId, revision: Sha256Digest, capacity: usize) -> Self {
        Self {
            session,
            lineage: Vec::new(),
            revision,
            capacity,
            records: BTreeMap::new(),
            pending: BTreeMap::new(),
            sealed: None,
            next_order: 0,
            outcomes: Vec::new(),
            closed: false,
            durability: Durability::Volatile,
        }
    }

    /// How new acceptances are held: [`Durability::Volatile`] unless the
    /// session journals them.
    #[must_use]
    pub const fn durability(&self) -> Durability {
        self.durability
    }

    /// Hold new acceptances as `durability`. A session that journals its
    /// commands sets [`Durability::WriteAhead`].
    pub const fn set_durability(&mut self, durability: Durability) {
        self.durability = durability;
    }

    /// An accepted command, as its client submitted it.
    #[must_use]
    pub fn command(&self, id: CommandId) -> Option<&Command> {
        self.records.get(&id).map(|record| &record.command)
    }

    /// The outcome at `cursor` in the outcome stream, with its command.
    #[must_use]
    pub fn outcome_at(&self, cursor: u64) -> Option<(CommandId, &Outcome)> {
        let id = *self.outcomes.get(usize::try_from(cursor).ok()?)?;
        match &self.records.get(&id)?.status {
            CommandStatus::Terminal(outcome) => Some((id, outcome)),
            _ => None,
        }
    }

    /// The session.
    #[must_use]
    pub const fn session(&self) -> SessionId {
        self.session
    }

    /// The forks this session descends through, root first; empty for a
    /// session that is not a branch.
    #[must_use]
    pub fn lineage(&self) -> &[Fork] {
        &self.lineage
    }

    /// The session that admitted order `order`.
    #[must_use]
    pub fn admitted_by(&self, order: u64) -> SessionId {
        admitting_session(&self.lineage, self.session, order)
    }

    /// This book, the closure of a checkpoint whose container's SHA-256 is
    /// `checkpoint` holding a world committed at `tick`, as the book of a
    /// new branch `session` (plan §14.4). The branch inherits every
    /// command, pending or ended, and the outcome stream, so its world
    /// continues exactly as this one would; admission numbering and the
    /// outcome cursor continue too. It admits only commands of `session`,
    /// and is open even if this book was closed.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when `session` is this session or one
    /// it descends from.
    pub fn branch(
        &self,
        session: SessionId,
        tick: Tick,
        checkpoint: Sha256Digest,
    ) -> Result<Self, CoreError> {
        if session == self.session || self.lineage.iter().any(|fork| fork.parent == session) {
            return Err(CoreError::StateConflict {
                operation: "branch session",
                detail: format!("{session} is already in the lineage of {}", self.session),
            });
        }
        let mut branch = self.clone();
        branch.lineage.push(Fork {
            parent: self.session,
            admitted: self.next_order,
            tick,
            checkpoint,
        });
        branch.session = session;
        branch.closed = false;
        branch.durability = Durability::Volatile;
        Ok(branch)
    }

    /// Everything a command journal would hold for this book: every
    /// acceptance in admission order and every outcome in cursor order.
    /// A branch's journal starts with its inherited history.
    #[must_use]
    pub fn history(&self) -> JournalContents {
        let mut accepted: Vec<(Command, Accepted)> = self
            .records
            .values()
            .map(|record| (record.command, record.accepted))
            .collect();
        accepted.sort_by_key(|(_, accepted)| accepted.order);
        let outcomes = self
            .outcomes
            .iter()
            .filter_map(|id| match &self.records.get(id)?.status {
                CommandStatus::Terminal(outcome) => Some((*id, outcome.clone())),
                _ => None,
            })
            .collect();
        JournalContents { accepted, outcomes }
    }

    /// The largest command identity the book holds, if any. A branch's own
    /// commands need identities its inherited ones do not use.
    #[must_use]
    pub fn last_command(&self) -> Option<CommandId> {
        self.records.keys().next_back().copied()
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
            durability: self.durability,
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

    /// Cancel `id` for `reason`: [`CommandBook::cancel`] with its reason
    /// stated, which a recovering session uses to replay a journaled
    /// cancellation at the boundary where it won.
    pub fn cancel_for(
        &mut self,
        id: CommandId,
        committed: Tick,
        reason: CancelReason,
    ) -> Cancellation {
        self.end(id, committed, reason)
    }

    /// Merge `journal`, everything a command journal holds for this session,
    /// into this book, the closure of a world committed at `committed` (a
    /// checkpoint's, or an empty book for the founding world).
    ///
    /// Every acceptance the book holds must be the journal's, and every
    /// outcome in its stream must be the journal's at the same cursor: the
    /// journal is the authority for commands, the checkpoint for the world.
    /// Each acceptance after the book's is added as pending, held
    /// [`Durability::WriteAhead`]. Returns the journal's outcomes after the
    /// book's stream, in cursor order: the outcomes the recovering session
    /// must reproduce, rather than record again, as it advances.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] naming the first disagreement: a record
    /// of another session, an admission order or cursor out of sequence, an
    /// acceptance or outcome that differs from the book's, a journal behind
    /// the book, a command due before `committed`, or an outcome at a
    /// boundary already passed or naming a command never accepted.
    pub fn recover(
        &mut self,
        journal: &JournalContents,
        committed: Tick,
    ) -> Result<Vec<(CommandId, Outcome)>, CoreError> {
        let conflict = |detail: String| CoreError::StateConflict {
            operation: "recover command book",
            detail,
        };
        for (position, (command, accepted)) in journal.accepted.iter().enumerate() {
            let position = u64::try_from(position).unwrap_or(u64::MAX);
            if accepted.order != position || command.session != self.admitted_by(position) {
                return Err(conflict(format!(
                    "journal acceptance {position} is {} of session {} in order {}",
                    command.id, command.session, accepted.order
                )));
            }
            let content = command.content_hash();
            if let Some(record) = self.records.get(&command.id) {
                if record.content != content
                    || record.accepted.order != accepted.order
                    || record.accepted.effective != accepted.effective
                    || record.accepted.retargeted != accepted.retargeted
                {
                    return Err(conflict(format!(
                        "{} differs between the journal and the checkpoint",
                        command.id
                    )));
                }
                continue;
            }
            if accepted.order != self.next_order {
                return Err(conflict(format!(
                    "journal acceptance {} ({}) is not in the checkpoint, which admitted {}",
                    accepted.order, command.id, self.next_order
                )));
            }
            if accepted.effective < committed {
                return Err(conflict(format!(
                    "{} is due at tick {}, before the committed tick {}",
                    command.id,
                    accepted.effective.get(),
                    committed.get()
                )));
            }
            self.pending
                .insert((accepted.effective, accepted.order), command.id);
            self.records.insert(
                command.id,
                Record {
                    content,
                    command: *command,
                    accepted: Accepted {
                        durability: Durability::WriteAhead,
                        ..*accepted
                    },
                    status: CommandStatus::Pending,
                },
            );
            self.next_order += 1;
        }
        let journaled = u64::try_from(journal.accepted.len()).unwrap_or(u64::MAX);
        if journaled < self.next_order {
            return Err(conflict(format!(
                "the journal holds {journaled} acceptances; the checkpoint admitted {}",
                self.next_order
            )));
        }
        if journal.outcomes.len() < self.outcomes.len() {
            return Err(conflict(format!(
                "the journal holds {} outcomes; the checkpoint's stream {}",
                journal.outcomes.len(),
                self.outcomes.len()
            )));
        }
        let mut replay = Vec::new();
        for (cursor, (id, outcome)) in journal.outcomes.iter().enumerate() {
            let at = u64::try_from(cursor).unwrap_or(u64::MAX);
            if outcome.cursor() != at {
                return Err(conflict(format!(
                    "journal outcome {at} is numbered {}",
                    outcome.cursor()
                )));
            }
            if let Some(&recorded) = self.outcomes.get(cursor) {
                if recorded != *id || self.outcome_at(at).map(|(_, held)| held) != Some(outcome) {
                    return Err(conflict(format!(
                        "outcome {at} differs between the journal and the checkpoint"
                    )));
                }
                continue;
            }
            let passed = match outcome {
                Outcome::Applied { tick, .. } => *tick < committed,
                Outcome::Cancelled { boundary, .. } => *boundary < committed,
                Outcome::Failed { .. } => false,
            };
            if passed || !self.records.contains_key(id) {
                return Err(conflict(format!(
                    "journal outcome {at} for {id} cannot be reached from tick {}",
                    committed.get()
                )));
            }
            replay.push((*id, outcome.clone()));
        }
        Ok(replay)
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

/// The command chunk's schema version for a session that is not a branch.
pub const COMMANDS_CHUNK_VERSION: u16 = 1;

/// The command chunk version that carries a lineage: a branch's.
pub const BRANCHED_COMMANDS_CHUNK_VERSION: u16 = 2;

/// The deepest lineage a checkpoint may carry.
const LINEAGE_MAX: usize = 1 << 16;

/// The longest command record or failure detail a checkpoint may carry.
const RECORD_MAX_BYTES: usize = 64 * 1024;

/// The most command records a checkpoint may carry.
const RECORDS_MAX: usize = 1 << 24;

fn refuse(reason: impl Into<String>) -> CoreError {
    CoreError::IncompatibleCheckpoint {
        reason: reason.into(),
    }
}

impl CommandBook {
    /// Add the book's command closure, [`ChunkFamily::Commands`], to
    /// `writer`: every accepted command with its canonical record, its
    /// acceptance and its status (pending, sealed or terminal with its
    /// outcome), the outcome stream, the admission counter, the sealed
    /// boundary and whether the book is closed. Pending future commands
    /// are carried whole, not as a cursor. A branch's chunk is
    /// [`BRANCHED_COMMANDS_CHUNK_VERSION`] and carries its lineage after
    /// the revision; any other book's is [`COMMANDS_CHUNK_VERSION`], byte
    /// for byte as before branches existed.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] if `writer` lacks the
    /// configuration chunk or already holds the commands.
    pub fn write_checkpoint(&self, writer: &mut CheckpointWriter<'_>) -> Result<(), CoreError> {
        let mut out = ByteWriter::new();
        out.u64(self.session.get());
        out.bytes(self.revision.as_bytes());
        if !self.lineage.is_empty() {
            out.u64(u64::try_from(self.lineage.len()).unwrap_or(u64::MAX));
            for fork in &self.lineage {
                out.u64(fork.parent.get());
                out.u64(fork.admitted);
                out.u64(fork.tick.get());
                out.bytes(fork.checkpoint.as_bytes());
            }
        }
        out.u64(u64::try_from(self.capacity).unwrap_or(u64::MAX));
        out.u64(self.next_order);
        out.option_u64(self.sealed.map(Tick::get));
        out.u8(u8::from(self.closed));
        out.u64(u64::try_from(self.records.len()).unwrap_or(u64::MAX));
        for record in self.records.values() {
            out.sized(&record.command.canonical_bytes());
            out.u64(record.accepted.order);
            out.u64(record.accepted.effective.get());
            out.u8(u8::from(record.accepted.retargeted));
            match record.accepted.durability {
                Durability::Volatile => out.u8(0),
                Durability::WriteAhead => out.u8(1),
            }
            match &record.status {
                CommandStatus::Pending => out.u8(0),
                CommandStatus::Sealed => out.u8(1),
                CommandStatus::Terminal(Outcome::Applied {
                    tick,
                    published,
                    epoch,
                    applied,
                    cursor,
                }) => {
                    out.u8(2);
                    out.u64(tick.get());
                    out.u64(published.get());
                    out.u64(epoch.get());
                    out.f64(*applied);
                    out.u64(*cursor);
                }
                CommandStatus::Terminal(Outcome::Cancelled {
                    boundary,
                    reason,
                    cursor,
                }) => {
                    out.u8(3);
                    out.u64(boundary.get());
                    out.u8(match reason {
                        CancelReason::Requested => 0,
                        CancelReason::SessionClosed => 1,
                    });
                    out.u64(*cursor);
                }
                CommandStatus::Terminal(Outcome::Failed { detail, cursor }) => {
                    out.u8(4);
                    out.sized(detail.as_bytes());
                    out.u64(*cursor);
                }
            }
        }
        out.u64(self.outcome_cursor());
        for id in &self.outcomes {
            out.u64(id.get());
        }
        writer.chunk(
            ChunkFamily::Commands,
            if self.lineage.is_empty() {
                COMMANDS_CHUNK_VERSION
            } else {
                BRANCHED_COMMANDS_CHUNK_VERSION
            },
            u64::try_from(self.records.len()).unwrap_or(u64::MAX),
            out.into_bytes(),
            &[ChunkFamily::Configuration],
        )
    }

    /// The book `reader`'s command closure holds, for a world committed at
    /// `committed` running configuration `revision`.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] for a closure that is not one
    /// this book could have reached: another revision; a record whose
    /// identity, content hash, admission order or status disagrees; a
    /// pending command due before `committed`; a sealed boundary other than
    /// `committed`; an outcome stream that is not exactly the terminal
    /// commands in cursor order; or a lineage that is not one: forks out of
    /// order, after the committed tick or past the commands admitted, a
    /// session repeated, or a command of a session that did not admit its
    /// order.
    pub fn read_checkpoint(
        reader: &CheckpointReader<'_>,
        committed: Tick,
        revision: Sha256Digest,
    ) -> Result<Self, CoreError> {
        let chunk = reader.chunk(ChunkFamily::Commands)?;
        let branched = match chunk.version {
            COMMANDS_CHUNK_VERSION => false,
            BRANCHED_COMMANDS_CHUNK_VERSION => true,
            other => {
                return Err(refuse(format!(
                    "chunk commands is version {other}; this build reads \
                     {COMMANDS_CHUNK_VERSION} and {BRANCHED_COMMANDS_CHUNK_VERSION}"
                )));
            }
        };
        let mut input = ByteReader::new(chunk.bytes, "commands chunk");
        let session = SessionId::new(input.u64()?);
        let recorded = Sha256Digest::from_bytes(input.array()?);
        if recorded != revision {
            return Err(refuse(format!(
                "the commands were accepted under configuration {}, the world runs {}",
                recorded.to_hex(),
                revision.to_hex()
            )));
        }
        let mut lineage = Vec::new();
        if branched {
            let forks = input.count(LINEAGE_MAX, 56)?;
            if forks == 0 {
                return Err(refuse("a branched commands chunk carries no fork"));
            }
            for _ in 0..forks {
                lineage.push(Fork {
                    parent: SessionId::new(input.u64()?),
                    admitted: input.u64()?,
                    tick: Tick::new(input.u64()?),
                    checkpoint: Sha256Digest::from_bytes(input.array()?),
                });
            }
        }
        let capacity = usize::try_from(input.u64()?).unwrap_or(usize::MAX);
        let next_order = input.u64()?;
        let mut sessions = BTreeSet::from([session]);
        let mut previous: Option<&Fork> = None;
        for fork in &lineage {
            if !sessions.insert(fork.parent)
                || fork.admitted > next_order
                || fork.tick > committed
                || previous.is_some_and(|before| {
                    fork.admitted < before.admitted || fork.tick < before.tick
                })
            {
                return Err(refuse(format!(
                    "the fork from {} at tick {} after {} admissions is not a lineage of \
                     {session} at tick {} with {next_order} admissions",
                    fork.parent,
                    fork.tick.get(),
                    fork.admitted,
                    committed.get()
                )));
            }
            previous = Some(fork);
        }
        let sealed = input.option_u64()?.map(Tick::new);
        if sealed.is_some_and(|sealed| sealed != committed) {
            return Err(refuse(format!(
                "boundary {sealed:?} is sealed in a world at tick {}",
                committed.get()
            )));
        }
        let closed = input.flag()?;
        let count = input.count(RECORDS_MAX, 8)?;
        let mut book = Self {
            session,
            lineage,
            revision,
            capacity,
            records: BTreeMap::new(),
            pending: BTreeMap::new(),
            sealed,
            next_order,
            outcomes: Vec::new(),
            closed,
            durability: Durability::Volatile,
        };
        let mut orders = BTreeSet::new();
        for _ in 0..count {
            let bytes = input.sized(RECORD_MAX_BYTES)?;
            let command = Command::from_canonical_bytes(bytes)?;
            let accepted = Accepted {
                order: input.u64()?,
                effective: Tick::new(input.u64()?),
                retargeted: input.flag()?,
                durability: match input.u8()? {
                    0 => Durability::Volatile,
                    1 => Durability::WriteAhead,
                    other => return Err(refuse(format!("unknown durability {other}"))),
                },
            };
            if command.session != book.admitted_by(accepted.order)
                || accepted.order >= next_order
                || !orders.insert(accepted.order)
            {
                return Err(refuse(format!(
                    "{} is not a command this session admitted in order {}",
                    command.id, accepted.order
                )));
            }
            let status = match input.u8()? {
                0 => CommandStatus::Pending,
                1 => CommandStatus::Sealed,
                2 => CommandStatus::Terminal(Outcome::Applied {
                    tick: Tick::new(input.u64()?),
                    published: Tick::new(input.u64()?),
                    epoch: ResourceEpoch::new(input.u64()?),
                    applied: input.f64()?,
                    cursor: input.u64()?,
                }),
                3 => CommandStatus::Terminal(Outcome::Cancelled {
                    boundary: Tick::new(input.u64()?),
                    reason: match input.u8()? {
                        0 => CancelReason::Requested,
                        1 => CancelReason::SessionClosed,
                        other => {
                            return Err(refuse(format!("unknown cancellation reason {other}")));
                        }
                    },
                    cursor: input.u64()?,
                }),
                4 => CommandStatus::Terminal(Outcome::Failed {
                    detail: std::str::from_utf8(input.sized(RECORD_MAX_BYTES)?)
                        .map_err(|_| refuse("a failure detail is not UTF-8"))?
                        .to_owned(),
                    cursor: input.u64()?,
                }),
                other => return Err(refuse(format!("unknown command status {other}"))),
            };
            match status {
                CommandStatus::Pending if accepted.effective < committed => {
                    return Err(refuse(format!(
                        "{} is pending for tick {}, before the committed tick {}",
                        command.id,
                        accepted.effective.get(),
                        committed.get()
                    )));
                }
                CommandStatus::Sealed if Some(accepted.effective) != sealed => {
                    return Err(refuse(format!(
                        "{} is sealed for tick {}, but {sealed:?} is sealed",
                        command.id,
                        accepted.effective.get()
                    )));
                }
                CommandStatus::Pending | CommandStatus::Sealed => {
                    book.pending
                        .insert((accepted.effective, accepted.order), command.id);
                }
                CommandStatus::Terminal(_) => {}
            }
            let id = command.id;
            let previous = book.records.insert(
                id,
                Record {
                    content: command.content_hash(),
                    command,
                    accepted,
                    status,
                },
            );
            if previous.is_some() {
                return Err(refuse(format!("{id} is recorded twice")));
            }
        }
        let outcomes = input.count(RECORDS_MAX, 8)?;
        for cursor in 0..outcomes {
            let id = CommandId::new(input.u64()?);
            let matches = book.records.get(&id).is_some_and(|record| {
                matches!(&record.status, CommandStatus::Terminal(outcome)
                    if outcome.cursor() == u64::try_from(cursor).unwrap_or(u64::MAX))
            });
            if !matches {
                return Err(refuse(format!(
                    "outcome {cursor} names {id}, which did not end there"
                )));
            }
            book.outcomes.push(id);
        }
        input.finish()?;
        if chunk.items != u64::try_from(book.records.len()).unwrap_or(u64::MAX) {
            return Err(refuse(format!(
                "the manifest counts {} command records, the chunk holds {}",
                chunk.items,
                book.records.len()
            )));
        }
        let terminal = book
            .records
            .values()
            .filter(|record| matches!(record.status, CommandStatus::Terminal(_)))
            .count();
        if terminal != book.outcomes.len() {
            return Err(refuse(format!(
                "{terminal} commands ended but the outcome stream holds {}",
                book.outcomes.len()
            )));
        }
        Ok(book)
    }
}
