//! The run catalog and command journal (FL-036; plan §12.3, §16.6,
//! Appendix B.1): one FrankenSQLite database file implementing
//! `flife_runtime`'s [`Journal`] port.
//!
//! Tables:
//! - `catalog`: the format marker, `frankenlife.run-catalog.v1`.
//! - `runs`: each session recorded, with the configuration hash it runs and
//!   the command schema.
//! - `commands`: each acceptance, keyed by `(session, admission order)` and
//!   unique by `(session, command identity)`, with its effective tick,
//!   retargeting, content hash and canonical record.
//! - `outcomes`: each terminal outcome, keyed by `(session, cursor)` and
//!   unique by `(session, command identity)`.
//! - `checkpoints`: an index of the chunk-store checkpoints published for a
//!   session. The chunk store's manifest stays the authority for whether a
//!   checkpoint is complete (§16.6): recovery loads it from the store, and a
//!   row here never makes a missing or damaged checkpoint resumable.
//! - `lineage` and `branches` (FL-039): for a branch session, each fork it
//!   descends through (root first) and where it was made: its name, the
//!   parent run, the parent checkpoint's sequence and the creation time.
//!   A branch's journal starts with the history it inherited
//!   ([`CommandJournal::open_branch`]); acceptances below a fork are of the
//!   session that admitted them, every later one is the branch's own.
//!
//! Every append is one transaction that checks the record is the next of
//! its kind, or exactly a record already held, before inserting it. The
//! database runs in WAL mode with `synchronous = FULL`, so each commit is
//! flushed before the append returns.

use std::path::{Path, PathBuf};

use flife_core::canonical::Sha256Digest;
use flife_core::ids::{CommandId, ResourceEpoch, SessionId};
use flife_core::{CoreError, Tick};
use flife_runtime::journal::{
    Journal, JournalContents, JournalRecord, Watermarks, block_on_inline,
};
use flife_runtime::{
    Accepted, COMMAND_SCHEMA, CancelReason, Command, Durability, Fork, Outcome, admitting_session,
};
use fsqlite::{Connection, FrankenError, SqliteValue};

/// The catalog's format marker.
pub const CATALOG_FORMAT: &str = "frankenlife.run-catalog.v1";

const SCHEMA: [&str; 10] = [
    "CREATE TABLE IF NOT EXISTS catalog (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS runs (session INTEGER PRIMARY KEY, revision BLOB NOT NULL, \
     command_schema TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS commands (session INTEGER NOT NULL, admission INTEGER NOT NULL, \
     command INTEGER NOT NULL, effective INTEGER NOT NULL, retargeted INTEGER NOT NULL, \
     content BLOB NOT NULL, record BLOB NOT NULL, PRIMARY KEY (session, admission))",
    "CREATE UNIQUE INDEX IF NOT EXISTS commands_identity ON commands (session, command)",
    "CREATE TABLE IF NOT EXISTS outcomes (session INTEGER NOT NULL, cursor INTEGER NOT NULL, \
     command INTEGER NOT NULL, kind TEXT NOT NULL, tick INTEGER, published INTEGER, \
     epoch INTEGER, applied_bits INTEGER, reason TEXT, detail TEXT, \
     PRIMARY KEY (session, cursor))",
    "CREATE UNIQUE INDEX IF NOT EXISTS outcomes_command ON outcomes (session, command)",
    "CREATE TABLE IF NOT EXISTS checkpoints (session INTEGER NOT NULL, sequence INTEGER NOT NULL, \
     tick INTEGER NOT NULL, epoch INTEGER NOT NULL, digest BLOB NOT NULL, \
     PRIMARY KEY (session, sequence))",
    "CREATE TABLE IF NOT EXISTS lineage (session INTEGER NOT NULL, depth INTEGER NOT NULL, \
     parent INTEGER NOT NULL, admitted INTEGER NOT NULL, tick INTEGER NOT NULL, \
     checkpoint BLOB NOT NULL, PRIMARY KEY (session, depth))",
    "CREATE TABLE IF NOT EXISTS branches (session INTEGER PRIMARY KEY, name TEXT NOT NULL, \
     parent_run TEXT NOT NULL, parent_sequence INTEGER NOT NULL, \
     created_unix_seconds INTEGER NOT NULL)",
    "INSERT OR IGNORE INTO catalog (key, value) VALUES ('format', 'frankenlife.run-catalog.v1')",
];

/// Where a branch run was made (Appendix B.1's identity): what
/// [`CommandJournal::open_branch`] records beside its lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchOrigin {
    /// The branch's name.
    pub name: String,
    /// The parent run, as it was given.
    pub parent_run: String,
    /// The parent checkpoint's sequence number in the parent's store.
    pub parent_sequence: u64,
    /// When the branch was made, in seconds since the Unix epoch.
    pub created_unix_seconds: u64,
}

/// A checkpoint the catalog indexes for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedCheckpoint {
    /// Its chunk-store sequence number.
    pub sequence: u64,
    /// The committed tick it holds.
    pub tick: Tick,
    /// The committed epoch it holds.
    pub epoch: ResourceEpoch,
    /// The SHA-256 of the whole container.
    pub digest: Sha256Digest,
}

/// The run catalog and command journal in one FrankenSQLite file.
pub struct CommandJournal {
    connection: Connection,
    path: PathBuf,
    session: Option<SessionId>,
}

impl std::fmt::Debug for CommandJournal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandJournal")
            .field("path", &self.path)
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

fn storage(operation: &'static str, path: &Path, error: &FrankenError) -> CoreError {
    CoreError::StorageFailed {
        operation,
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}

/// A `u64` as the SQL integer with the same bits.
fn int(value: u64) -> SqliteValue {
    SqliteValue::Integer(i64::from_le_bytes(value.to_le_bytes()))
}

/// The `u64` whose bits an SQL integer holds.
const fn unsigned(value: i64) -> u64 {
    u64::from_le_bytes(value.to_le_bytes())
}

fn blob(bytes: &[u8]) -> SqliteValue {
    SqliteValue::from(bytes)
}

fn text(value: &str) -> SqliteValue {
    SqliteValue::from(value)
}

fn malformed(detail: impl Into<String>) -> CoreError {
    CoreError::IncompatibleCheckpoint {
        reason: format!("command journal: {}", detail.into()),
    }
}

fn column(row: &fsqlite::Row, index: usize) -> &SqliteValue {
    row.get(index).unwrap_or(&SqliteValue::Null)
}

fn integer(row: &fsqlite::Row, index: usize) -> Result<u64, CoreError> {
    column(row, index)
        .as_integer()
        .map(unsigned)
        .ok_or_else(|| malformed(format!("column {index} is not an integer")))
}

fn optional_integer(row: &fsqlite::Row, index: usize) -> Result<Option<u64>, CoreError> {
    match column(row, index) {
        SqliteValue::Null => Ok(None),
        value => value
            .as_integer()
            .map(|value| Some(unsigned(value)))
            .ok_or_else(|| malformed(format!("column {index} is not an integer"))),
    }
}

fn bytes(row: &fsqlite::Row, index: usize) -> Result<&[u8], CoreError> {
    column(row, index)
        .as_blob()
        .ok_or_else(|| malformed(format!("column {index} is not a blob")))
}

fn string(row: &fsqlite::Row, index: usize) -> Result<Option<&str>, CoreError> {
    match column(row, index) {
        SqliteValue::Null => Ok(None),
        value => value
            .as_text()
            .map(Some)
            .ok_or_else(|| malformed(format!("column {index} is not text"))),
    }
}

fn digest(row: &fsqlite::Row, index: usize) -> Result<Sha256Digest, CoreError> {
    let held = bytes(row, index)?;
    let array: [u8; 32] = held
        .try_into()
        .map_err(|_| malformed(format!("column {index} is not a SHA-256")))?;
    Ok(Sha256Digest::from_bytes(array))
}

/// An outcome's columns: kind, tick, published, epoch, applied bits,
/// reason and detail.
fn outcome_columns(outcome: &Outcome) -> [SqliteValue; 7] {
    match outcome {
        Outcome::Applied {
            tick,
            published,
            epoch,
            applied,
            ..
        } => [
            text("applied"),
            int(tick.get()),
            int(published.get()),
            int(epoch.get()),
            int(applied.to_bits()),
            SqliteValue::Null,
            SqliteValue::Null,
        ],
        Outcome::Cancelled {
            boundary, reason, ..
        } => [
            text("cancelled"),
            int(boundary.get()),
            SqliteValue::Null,
            SqliteValue::Null,
            SqliteValue::Null,
            text(match reason {
                CancelReason::Requested => "requested",
                CancelReason::SessionClosed => "session-closed",
            }),
            SqliteValue::Null,
        ],
        Outcome::Failed { detail, .. } => [
            text("failed"),
            SqliteValue::Null,
            SqliteValue::Null,
            SqliteValue::Null,
            SqliteValue::Null,
            SqliteValue::Null,
            text(detail),
        ],
    }
}

/// The outcome at `cursor` whose columns start at `first` in `row`.
fn read_outcome(row: &fsqlite::Row, first: usize, cursor: u64) -> Result<Outcome, CoreError> {
    let kind = string(row, first)?.unwrap_or_default();
    let tick = || {
        optional_integer(row, first + 1)?
            .map(Tick::new)
            .ok_or_else(|| malformed(format!("outcome {cursor} has no tick")))
    };
    match kind {
        "applied" => Ok(Outcome::Applied {
            tick: tick()?,
            published: Tick::new(integer(row, first + 2)?),
            epoch: ResourceEpoch::new(integer(row, first + 3)?),
            applied: f64::from_bits(integer(row, first + 4)?),
            cursor,
        }),
        "cancelled" => Ok(Outcome::Cancelled {
            boundary: tick()?,
            reason: match string(row, first + 5)? {
                Some("requested") => CancelReason::Requested,
                Some("session-closed") => CancelReason::SessionClosed,
                other => return Err(malformed(format!("cancellation reason {other:?}"))),
            },
            cursor,
        }),
        "failed" => Ok(Outcome::Failed {
            detail: string(row, first + 6)?.unwrap_or_default().to_owned(),
            cursor,
        }),
        other => Err(malformed(format!("outcome kind {other:?}"))),
    }
}

impl CommandJournal {
    /// Open the catalog at `path`, creating it if it does not exist, in WAL
    /// mode with `synchronous = FULL`.
    ///
    /// # Errors
    /// [`CoreError::StorageFailed`] when the database cannot be opened or
    /// its schema created; [`CoreError::SchemaMismatch`] for a database of
    /// another catalog format.
    pub fn open(path: &Path) -> Result<Self, CoreError> {
        let location = path.display().to_string();
        block_on_inline(async {
            let fail = |operation, error: &FrankenError| storage(operation, path, error);
            let connection = Connection::open(location.clone())
                .await
                .map_err(|error| fail("open run catalog", &error))?;
            connection
                .query("PRAGMA journal_mode = WAL")
                .await
                .map_err(|error| fail("set journal mode", &error))?;
            connection
                .execute("PRAGMA synchronous = FULL")
                .await
                .map_err(|error| fail("set synchronous mode", &error))?;
            for statement in SCHEMA {
                connection
                    .execute(statement)
                    .await
                    .map_err(|error| fail("create run catalog schema", &error))?;
            }
            let format = connection
                .query("SELECT value FROM catalog WHERE key = 'format'")
                .await
                .map_err(|error| fail("read catalog format", &error))?;
            let found = format
                .first()
                .and_then(|row| row.get(0))
                .and_then(SqliteValue::as_text)
                .map(str::to_owned);
            if found.as_deref() != Some(CATALOG_FORMAT) {
                return Err(CoreError::SchemaMismatch {
                    expected: CATALOG_FORMAT.into(),
                    found: found.unwrap_or_default(),
                });
            }
            Ok(Self {
                connection,
                path: path.to_path_buf(),
                session: None,
            })
        })
    }

    /// The database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The session [`Journal::open_run`] bound, if any.
    #[must_use]
    pub const fn session(&self) -> Option<SessionId> {
        self.session
    }

    /// The journal mode and `synchronous` level the connection reports.
    ///
    /// # Errors
    /// [`CoreError::StorageFailed`] if they cannot be read.
    pub fn modes(&self) -> Result<(String, i64), CoreError> {
        block_on_inline(async {
            let fail = |error: &FrankenError| storage("read journal modes", &self.path, error);
            let journal = self
                .connection
                .query("PRAGMA journal_mode")
                .await
                .map_err(|error| fail(&error))?;
            let synchronous = self
                .connection
                .query("PRAGMA synchronous")
                .await
                .map_err(|error| fail(&error))?;
            let journal = journal
                .first()
                .and_then(|row| row.get(0))
                .and_then(SqliteValue::as_text)
                .unwrap_or_default()
                .to_owned();
            let synchronous = synchronous
                .first()
                .and_then(|row| row.get(0))
                .and_then(SqliteValue::as_integer)
                .unwrap_or(-1);
            Ok((journal, synchronous))
        })
    }

    fn bound(&self) -> Result<SessionId, CoreError> {
        self.session.ok_or_else(|| CoreError::StateConflict {
            operation: "append to command journal",
            detail: "no run is open: call open_run first".into(),
        })
    }

    /// Run `body` as one transaction: commit if it succeeds, roll back if it
    /// fails.
    async fn transaction<T>(
        &self,
        operation: &'static str,
        body: impl AsyncFnOnce(&Connection) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        self.connection
            .execute("BEGIN")
            .await
            .map_err(|error| storage(operation, &self.path, &error))?;
        match body(&self.connection).await {
            Ok(value) => {
                self.connection
                    .execute("COMMIT")
                    .await
                    .map_err(|error| storage(operation, &self.path, &error))?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.connection.execute("ROLLBACK").await;
                Err(error)
            }
        }
    }

    async fn count(
        connection: &Connection,
        path: &Path,
        table: &str,
        session: SessionId,
    ) -> Result<u64, CoreError> {
        let rows = connection
            .query_with_params(
                &format!("SELECT COUNT(*) FROM {table} WHERE session = ?1"),
                &[int(session.get())],
            )
            .await
            .map_err(|error| storage("count journal records", path, &error))?;
        rows.first().map_or(Ok(0), |row| integer(row, 0))
    }

    async fn watermarks_in(
        connection: &Connection,
        path: &Path,
        session: SessionId,
    ) -> Result<Watermarks, CoreError> {
        Ok(Watermarks {
            admitted: Self::count(connection, path, "commands", session).await?,
            outcomes: Self::count(connection, path, "outcomes", session).await?,
        })
    }

    /// The durable watermarks of the bound session.
    ///
    /// # Errors
    /// As [`Journal::append`].
    pub fn watermarks(&self) -> Result<Watermarks, CoreError> {
        let session = self.bound()?;
        block_on_inline(Self::watermarks_in(&self.connection, &self.path, session))
    }

    async fn append_accepted(
        &self,
        session: SessionId,
        command: &Command,
        accepted: &Accepted,
    ) -> Result<Watermarks, CoreError> {
        let path = self.path.clone();
        let record = command.canonical_bytes();
        let content = command.content_hash();
        self.transaction("append acceptance", async |connection| {
            let read = |error: &FrankenError| storage("read journal acceptance", &path, error);
            let held = connection
                .query_with_params(
                    "SELECT admission, effective, retargeted, content FROM commands \
                     WHERE session = ?1 AND command = ?2",
                    &[int(session.get()), int(command.id.get())],
                )
                .await
                .map_err(|error| read(&error))?;
            if let Some(row) = held.first() {
                let same = integer(row, 0)? == accepted.order
                    && integer(row, 1)? == accepted.effective.get()
                    && integer(row, 2)? == u64::from(accepted.retargeted)
                    && digest(row, 3)? == content;
                if !same {
                    return Err(CoreError::StateConflict {
                        operation: "append acceptance",
                        detail: format!(
                            "{} is journaled with other content or another admission",
                            command.id
                        ),
                    });
                }
                return Self::watermarks_in(connection, &path, session).await;
            }
            let admitted = Self::count(connection, &path, "commands", session).await?;
            if accepted.order != admitted {
                return Err(CoreError::StateConflict {
                    operation: "append acceptance",
                    detail: format!(
                        "{} has admission order {}, but the journal holds {admitted} acceptances",
                        command.id, accepted.order
                    ),
                });
            }
            connection
                .execute_with_params(
                    "INSERT INTO commands (session, admission, command, effective, retargeted, \
                     content, record) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    &[
                        int(session.get()),
                        int(accepted.order),
                        int(command.id.get()),
                        int(accepted.effective.get()),
                        int(u64::from(accepted.retargeted)),
                        blob(content.as_bytes()),
                        blob(&record),
                    ],
                )
                .await
                .map_err(|error| storage("insert journal acceptance", &path, &error))?;
            Self::watermarks_in(connection, &path, session).await
        })
        .await
    }

    async fn append_ended(
        &self,
        session: SessionId,
        id: CommandId,
        outcome: &Outcome,
    ) -> Result<Watermarks, CoreError> {
        let path = self.path.clone();
        let cursor = outcome.cursor();
        self.transaction("append outcome", async |connection| {
            let read = |error: &FrankenError| storage("read journal outcome", &path, error);
            let accepted = connection
                .query_with_params(
                    "SELECT admission FROM commands WHERE session = ?1 AND command = ?2",
                    &[int(session.get()), int(id.get())],
                )
                .await
                .map_err(|error| read(&error))?;
            if accepted.is_empty() {
                return Err(CoreError::StateConflict {
                    operation: "append outcome",
                    detail: format!("{id} ended, but the journal never accepted it"),
                });
            }
            let held = connection
                .query_with_params(
                    "SELECT cursor, kind, tick, published, epoch, applied_bits, reason, detail \
                     FROM outcomes WHERE session = ?1 AND command = ?2",
                    &[int(session.get()), int(id.get())],
                )
                .await
                .map_err(|error| read(&error))?;
            if let Some(row) = held.first() {
                let at = integer(row, 0)?;
                if at != cursor || read_outcome(row, 1, at)? != *outcome {
                    return Err(CoreError::StateConflict {
                        operation: "append outcome",
                        detail: format!("{id} is journaled with another outcome"),
                    });
                }
                return Self::watermarks_in(connection, &path, session).await;
            }
            let outcomes = Self::count(connection, &path, "outcomes", session).await?;
            if cursor != outcomes {
                return Err(CoreError::StateConflict {
                    operation: "append outcome",
                    detail: format!(
                        "{id} ended at cursor {cursor}, but the journal holds {outcomes} outcomes"
                    ),
                });
            }
            let [kind, tick, published, epoch, applied, reason, detail] = outcome_columns(outcome);
            connection
                .execute_with_params(
                    "INSERT INTO outcomes (session, cursor, command, kind, tick, published, \
                     epoch, applied_bits, reason, detail) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    &[
                        int(session.get()),
                        int(cursor),
                        int(id.get()),
                        kind,
                        tick,
                        published,
                        epoch,
                        applied,
                        reason,
                        detail,
                    ],
                )
                .await
                .map_err(|error| storage("insert journal outcome", &path, &error))?;
            Self::watermarks_in(connection, &path, session).await
        })
        .await
    }

    async fn lineage_of(&self, session: SessionId) -> Result<Vec<Fork>, CoreError> {
        let rows = self
            .connection
            .query_with_params(
                "SELECT depth, parent, admitted, tick, checkpoint FROM lineage \
                 WHERE session = ?1 ORDER BY depth",
                &[int(session.get())],
            )
            .await
            .map_err(|error| storage("read lineage", &self.path, &error))?;
        rows.iter()
            .enumerate()
            .map(|(position, row)| {
                if integer(row, 0)? != u64::try_from(position).unwrap_or(u64::MAX) {
                    return Err(malformed(format!("lineage depth {position} is missing")));
                }
                Ok(Fork {
                    parent: SessionId::new(integer(row, 1)?),
                    admitted: integer(row, 2)?,
                    tick: Tick::new(integer(row, 3)?),
                    checkpoint: digest(row, 4)?,
                })
            })
            .collect()
    }

    async fn contents(&self, session: SessionId) -> Result<JournalContents, CoreError> {
        let fail = |error: &FrankenError| storage("read command journal", &self.path, error);
        let lineage = self.lineage_of(session).await?;
        let mut contents = JournalContents::default();
        let rows = self
            .connection
            .query_with_params(
                "SELECT admission, command, effective, retargeted, content, record \
                 FROM commands WHERE session = ?1 ORDER BY admission",
                &[int(session.get())],
            )
            .await
            .map_err(|error| fail(&error))?;
        for (position, row) in rows.iter().enumerate() {
            let order = integer(row, 0)?;
            let command = Command::from_canonical_bytes(bytes(row, 5)?)?;
            if order != u64::try_from(position).unwrap_or(u64::MAX)
                || command.id.get() != integer(row, 1)?
                || command.session != admitting_session(&lineage, session, order)
                || command.content_hash() != digest(row, 4)?
            {
                return Err(malformed(format!(
                    "acceptance {position} does not match its record"
                )));
            }
            let accepted = Accepted {
                order,
                effective: Tick::new(integer(row, 2)?),
                retargeted: match integer(row, 3)? {
                    0 => false,
                    1 => true,
                    other => return Err(malformed(format!("retargeted flag {other}"))),
                },
                durability: Durability::WriteAhead,
            };
            contents.accepted.push((command, accepted));
        }
        let rows = self
            .connection
            .query_with_params(
                "SELECT cursor, command, kind, tick, published, epoch, applied_bits, reason, \
                 detail FROM outcomes WHERE session = ?1 ORDER BY cursor",
                &[int(session.get())],
            )
            .await
            .map_err(|error| fail(&error))?;
        for (position, row) in rows.iter().enumerate() {
            let cursor = integer(row, 0)?;
            if cursor != u64::try_from(position).unwrap_or(u64::MAX) {
                return Err(malformed(format!(
                    "outcome {position} is numbered {cursor}"
                )));
            }
            contents.outcomes.push((
                CommandId::new(integer(row, 1)?),
                read_outcome(row, 2, cursor)?,
            ));
        }
        Ok(contents)
    }

    /// Index checkpoint `sequence` of the bound session, which a chunk store
    /// has published, holding committed `tick` and `epoch` in a container
    /// whose SHA-256 is `digest`. Indexing it again with the same values
    /// writes nothing.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when no run is open or the sequence is
    /// indexed with other values; [`CoreError::StorageFailed`] when the host
    /// refuses.
    pub fn index_checkpoint(&mut self, checkpoint: IndexedCheckpoint) -> Result<(), CoreError> {
        let session = self.bound()?;
        let path = self.path.clone();
        block_on_inline(self.transaction("index checkpoint", async |connection| {
            let held = connection
                .query_with_params(
                    "SELECT tick, epoch, digest FROM checkpoints \
                     WHERE session = ?1 AND sequence = ?2",
                    &[int(session.get()), int(checkpoint.sequence)],
                )
                .await
                .map_err(|error| storage("read checkpoint index", &path, &error))?;
            if let Some(row) = held.first() {
                if integer(row, 0)? != checkpoint.tick.get()
                    || integer(row, 1)? != checkpoint.epoch.get()
                    || digest(row, 2)? != checkpoint.digest
                {
                    return Err(CoreError::StateConflict {
                        operation: "index checkpoint",
                        detail: format!(
                            "checkpoint {} is indexed with other values",
                            checkpoint.sequence
                        ),
                    });
                }
                return Ok(());
            }
            connection
                .execute_with_params(
                    "INSERT INTO checkpoints (session, sequence, tick, epoch, digest) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    &[
                        int(session.get()),
                        int(checkpoint.sequence),
                        int(checkpoint.tick.get()),
                        int(checkpoint.epoch.get()),
                        blob(checkpoint.digest.as_bytes()),
                    ],
                )
                .await
                .map_err(|error| storage("insert checkpoint index", &path, &error))?;
            Ok(())
        }))
    }

    /// The bound session's indexed checkpoints, in sequence order.
    ///
    /// # Errors
    /// As [`CommandJournal::index_checkpoint`].
    pub fn checkpoints(&self) -> Result<Vec<IndexedCheckpoint>, CoreError> {
        let session = self.bound()?;
        block_on_inline(async {
            let rows = self
                .connection
                .query_with_params(
                    "SELECT sequence, tick, epoch, digest FROM checkpoints \
                     WHERE session = ?1 ORDER BY sequence",
                    &[int(session.get())],
                )
                .await
                .map_err(|error| storage("read checkpoint index", &self.path, &error))?;
            rows.iter()
                .map(|row| {
                    Ok(IndexedCheckpoint {
                        sequence: integer(row, 0)?,
                        tick: Tick::new(integer(row, 1)?),
                        epoch: ResourceEpoch::new(integer(row, 2)?),
                        digest: digest(row, 3)?,
                    })
                })
                .collect()
        })
    }

    /// Record branch `session` running configuration `revision`, forked
    /// through `lineage` (root first; the last fork is its own) at
    /// `origin`, and start its journal with `history`, the commands and
    /// outcomes it inherited (empty for a session that journals nothing),
    /// all in one transaction. Then bind the session, as
    /// [`Journal::open_run`] does.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when the catalog already records
    /// `session`, `lineage` is empty, or `history` is not numbered from
    /// zero with each acceptance of the session that admitted its order;
    /// [`CoreError::StorageFailed`] when the host refuses.
    pub fn open_branch(
        &mut self,
        session: SessionId,
        revision: Sha256Digest,
        lineage: &[Fork],
        origin: &BranchOrigin,
        history: &JournalContents,
    ) -> Result<(), CoreError> {
        let conflict = |detail: String| CoreError::StateConflict {
            operation: "record branch",
            detail,
        };
        if lineage.is_empty() {
            return Err(conflict(format!("{session} has no fork")));
        }
        for (position, (command, accepted)) in history.accepted.iter().enumerate() {
            let order = u64::try_from(position).unwrap_or(u64::MAX);
            if accepted.order != order
                || command.session != admitting_session(lineage, session, order)
            {
                return Err(conflict(format!(
                    "inherited acceptance {position} is {} of {} in order {}",
                    command.id, command.session, accepted.order
                )));
            }
        }
        for (position, (id, outcome)) in history.outcomes.iter().enumerate() {
            if outcome.cursor() != u64::try_from(position).unwrap_or(u64::MAX) {
                return Err(conflict(format!(
                    "inherited outcome {position} ({id}) is numbered {}",
                    outcome.cursor()
                )));
            }
        }
        let path = self.path.clone();
        block_on_inline(self.transaction("record branch", async |connection| {
            let insert = |error: &FrankenError| storage("record branch", &path, error);
            let held = connection
                .query_with_params(
                    "SELECT session FROM runs WHERE session = ?1",
                    &[int(session.get())],
                )
                .await
                .map_err(|error| insert(&error))?;
            if !held.is_empty() {
                return Err(CoreError::StateConflict {
                    operation: "record branch",
                    detail: format!("the catalog already records {session}"),
                });
            }
            connection
                .execute_with_params(
                    "INSERT INTO runs (session, revision, command_schema) VALUES (?1, ?2, ?3)",
                    &[
                        int(session.get()),
                        blob(revision.as_bytes()),
                        text(COMMAND_SCHEMA),
                    ],
                )
                .await
                .map_err(|error| insert(&error))?;
            for (depth, fork) in (0_u64..).zip(lineage) {
                connection
                    .execute_with_params(
                        "INSERT INTO lineage (session, depth, parent, admitted, tick, \
                         checkpoint) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        &[
                            int(session.get()),
                            int(depth),
                            int(fork.parent.get()),
                            int(fork.admitted),
                            int(fork.tick.get()),
                            blob(fork.checkpoint.as_bytes()),
                        ],
                    )
                    .await
                    .map_err(|error| insert(&error))?;
            }
            connection
                .execute_with_params(
                    "INSERT INTO branches (session, name, parent_run, parent_sequence, \
                     created_unix_seconds) VALUES (?1, ?2, ?3, ?4, ?5)",
                    &[
                        int(session.get()),
                        text(&origin.name),
                        text(&origin.parent_run),
                        int(origin.parent_sequence),
                        int(origin.created_unix_seconds),
                    ],
                )
                .await
                .map_err(|error| insert(&error))?;
            for (command, accepted) in &history.accepted {
                connection
                    .execute_with_params(
                        "INSERT INTO commands (session, admission, command, effective, \
                         retargeted, content, record) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        &[
                            int(session.get()),
                            int(accepted.order),
                            int(command.id.get()),
                            int(accepted.effective.get()),
                            int(u64::from(accepted.retargeted)),
                            blob(command.content_hash().as_bytes()),
                            blob(&command.canonical_bytes()),
                        ],
                    )
                    .await
                    .map_err(|error| insert(&error))?;
            }
            for (id, outcome) in &history.outcomes {
                let [kind, tick, published, epoch, applied, reason, detail] =
                    outcome_columns(outcome);
                connection
                    .execute_with_params(
                        "INSERT INTO outcomes (session, cursor, command, kind, tick, published, \
                         epoch, applied_bits, reason, detail) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        &[
                            int(session.get()),
                            int(outcome.cursor()),
                            int(id.get()),
                            kind,
                            tick,
                            published,
                            epoch,
                            applied,
                            reason,
                            detail,
                        ],
                    )
                    .await
                    .map_err(|error| insert(&error))?;
            }
            Ok(())
        }))?;
        self.session = Some(session);
        Ok(())
    }

    /// The bound session's lineage, root first: empty unless it is a
    /// branch.
    ///
    /// # Errors
    /// As [`CommandJournal::index_checkpoint`].
    pub fn lineage(&self) -> Result<Vec<Fork>, CoreError> {
        let session = self.bound()?;
        block_on_inline(self.lineage_of(session))
    }

    /// Where the bound session was branched, if it is a branch.
    ///
    /// # Errors
    /// As [`CommandJournal::index_checkpoint`].
    pub fn branch_origin(&self) -> Result<Option<BranchOrigin>, CoreError> {
        let session = self.bound()?;
        block_on_inline(async {
            let rows = self
                .connection
                .query_with_params(
                    "SELECT name, parent_run, parent_sequence, created_unix_seconds \
                     FROM branches WHERE session = ?1",
                    &[int(session.get())],
                )
                .await
                .map_err(|error| storage("read branch", &self.path, &error))?;
            rows.first()
                .map(|row| {
                    Ok(BranchOrigin {
                        name: string(row, 0)?.unwrap_or_default().to_owned(),
                        parent_run: string(row, 1)?.unwrap_or_default().to_owned(),
                        parent_sequence: integer(row, 2)?,
                        created_unix_seconds: integer(row, 3)?,
                    })
                })
                .transpose()
        })
    }

    /// The sessions the catalog records, with the configuration hash each
    /// runs, in session order.
    ///
    /// # Errors
    /// [`CoreError::StorageFailed`] when the host refuses.
    pub fn runs(&self) -> Result<Vec<(SessionId, Sha256Digest)>, CoreError> {
        block_on_inline(async {
            let rows = self
                .connection
                .query("SELECT session, revision FROM runs ORDER BY session")
                .await
                .map_err(|error| storage("read run catalog", &self.path, &error))?;
            rows.iter()
                .map(|row| Ok((SessionId::new(integer(row, 0)?), digest(row, 1)?)))
                .collect()
        })
    }
}

impl Journal for CommandJournal {
    fn open_run(
        &mut self,
        session: SessionId,
        revision: Sha256Digest,
    ) -> Result<JournalContents, CoreError> {
        let path = self.path.clone();
        block_on_inline(async {
            self.transaction("open run", async |connection| {
                let held = connection
                    .query_with_params(
                        "SELECT revision, command_schema FROM runs WHERE session = ?1",
                        &[int(session.get())],
                    )
                    .await
                    .map_err(|error| storage("read run catalog", &path, &error))?;
                match held.first() {
                    Some(row) => {
                        let recorded = digest(row, 0)?;
                        let schema = string(row, 1)?.unwrap_or_default();
                        if recorded != revision || schema != COMMAND_SCHEMA {
                            return Err(CoreError::StateConflict {
                                operation: "open run",
                                detail: format!(
                                    "session {session} is recorded running configuration {} \
                                     with {schema}, not {} with {COMMAND_SCHEMA}",
                                    recorded.to_hex(),
                                    revision.to_hex()
                                ),
                            });
                        }
                    }
                    None => {
                        connection
                            .execute_with_params(
                                "INSERT INTO runs (session, revision, command_schema) \
                                 VALUES (?1, ?2, ?3)",
                                &[
                                    int(session.get()),
                                    blob(revision.as_bytes()),
                                    text(COMMAND_SCHEMA),
                                ],
                            )
                            .await
                            .map_err(|error| storage("record run", &path, &error))?;
                    }
                }
                Ok(())
            })
            .await?;
            let contents = self.contents(session).await?;
            self.session = Some(session);
            Ok(contents)
        })
    }

    fn append(&mut self, record: &JournalRecord) -> Result<Watermarks, CoreError> {
        let session = self.bound()?;
        block_on_inline(async {
            match record {
                JournalRecord::Accepted { command, accepted } => {
                    if command.session != session {
                        return Err(CoreError::StateConflict {
                            operation: "append acceptance",
                            detail: format!(
                                "{} is addressed to session {}, the journal holds {session}",
                                command.id, command.session
                            ),
                        });
                    }
                    self.append_accepted(session, command, accepted).await
                }
                JournalRecord::Ended { id, outcome } => {
                    self.append_ended(session, *id, outcome).await
                }
            }
        })
    }
}

impl Drop for CommandJournal {
    fn drop(&mut self) {
        block_on_inline(self.connection.close_best_effort_in_place());
    }
}
