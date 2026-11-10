//! The experiment catalog (FL-041; plan §15.3–§15.5, §21.6, Appendix B.1).
//!
//! One FrankenSQLite file per experiment directory: the experiment's
//! identity and design, and one row per run, written when the run ends,
//! whether it reached its tick, died out, paused at a capacity limit or
//! failed. A run is cataloged once; an experiment continued after a kill
//! skips the runs already cataloged and runs the rest. The catalog stores
//! each run's result and manifest as the caller's JSON text and never
//! interprets them.

use std::path::{Path, PathBuf};

use flife_core::CoreError;
use flife_runtime::journal::block_on_inline;
use fsqlite::{Connection, FrankenError, SqliteValue};

/// The experiment catalog's format marker.
pub const EXPERIMENT_CATALOG_FORMAT: &str = "frankenlife.experiment-catalog.v1";

/// The format marker, created first so a database of another format is
/// refused before anything else is written to it.
const MARKER: [&str; 2] = [
    "CREATE TABLE IF NOT EXISTS catalog (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    "INSERT OR IGNORE INTO catalog (key, value) \
     VALUES ('format', 'frankenlife.experiment-catalog.v1')",
];

const SCHEMA: [&str; 2] = [
    "CREATE TABLE IF NOT EXISTS runs (ordinal INTEGER PRIMARY KEY, condition TEXT NOT NULL, \
     split TEXT NOT NULL, seed INTEGER NOT NULL, config_hash TEXT NOT NULL, \
     status TEXT NOT NULL, result TEXT NOT NULL, manifest TEXT NOT NULL, \
     records_sha256 TEXT NOT NULL)",
    "CREATE UNIQUE INDEX IF NOT EXISTS runs_identity ON runs (condition, split, seed)",
];

/// One cataloged run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogedRun {
    /// Its place in the experiment's run order, from 1.
    pub ordinal: u64,
    /// The condition it ran.
    pub condition: String,
    /// `discovery` or `held-out`.
    pub split: String,
    /// The world seed.
    pub seed: u64,
    /// The run configuration's canonical hash.
    pub config_hash: String,
    /// How it ended: `reached`, `capacity-paused`, `failed` or `error`.
    pub status: String,
    /// Its scientific result, as the caller's JSON.
    pub result: String,
    /// Its run manifest (Appendix B.1), as the caller's JSON.
    pub manifest: String,
    /// The SHA-256 of the run's record file.
    pub records_sha256: String,
}

/// An experiment directory's catalog.
pub struct ExperimentCatalog {
    connection: Connection,
    path: PathBuf,
}

impl std::fmt::Debug for ExperimentCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExperimentCatalog")
            .field("path", &self.path)
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

fn malformed(detail: impl Into<String>) -> CoreError {
    CoreError::StateConflict {
        operation: "read experiment catalog",
        detail: detail.into(),
    }
}

/// A `u64` as the SQL integer with the same bits.
fn int(value: u64) -> SqliteValue {
    SqliteValue::Integer(i64::from_le_bytes(value.to_le_bytes()))
}

fn text(value: &str) -> SqliteValue {
    SqliteValue::from(value)
}

fn integer(row: &fsqlite::Row, index: usize) -> Result<u64, CoreError> {
    row.get(index)
        .and_then(SqliteValue::as_integer)
        .map(|value| u64::from_le_bytes(value.to_le_bytes()))
        .ok_or_else(|| malformed(format!("column {index} is not an integer")))
}

fn string(row: &fsqlite::Row, index: usize) -> Result<String, CoreError> {
    row.get(index)
        .and_then(SqliteValue::as_text)
        .map(str::to_owned)
        .ok_or_else(|| malformed(format!("column {index} is not text")))
}

impl ExperimentCatalog {
    /// Open the catalog at `path` for the experiment whose identity is
    /// `experiment` and whose design is `design`, creating it if it does
    /// not exist, in WAL mode with `synchronous = FULL`.
    ///
    /// # Errors
    /// [`CoreError::StorageFailed`] when the database cannot be opened or
    /// written; [`CoreError::SchemaMismatch`] for a database of another
    /// format; [`CoreError::StateConflict`] when it catalogs another
    /// experiment.
    pub fn open(path: &Path, experiment: &str, design: &str) -> Result<Self, CoreError> {
        let location = path.display().to_string();
        let connection = block_on_inline(async {
            let fail = |operation, error: &FrankenError| storage(operation, path, error);
            let connection = Connection::open(location)
                .await
                .map_err(|error| fail("open experiment catalog", &error))?;
            connection
                .query("PRAGMA journal_mode = WAL")
                .await
                .map_err(|error| fail("set journal mode", &error))?;
            connection
                .execute("PRAGMA synchronous = FULL")
                .await
                .map_err(|error| fail("set synchronous mode", &error))?;
            for statement in MARKER {
                connection
                    .execute(statement)
                    .await
                    .map_err(|error| fail("create experiment catalog schema", &error))?;
            }
            Ok::<_, CoreError>(connection)
        })?;
        let catalog = Self {
            connection,
            path: path.to_path_buf(),
        };
        let format = catalog.value("format")?;
        if format.as_deref() != Some(EXPERIMENT_CATALOG_FORMAT) {
            return Err(CoreError::SchemaMismatch {
                expected: EXPERIMENT_CATALOG_FORMAT.into(),
                found: format.unwrap_or_default(),
            });
        }
        block_on_inline(async {
            for statement in SCHEMA {
                catalog
                    .connection
                    .execute(statement)
                    .await
                    .map_err(|error| storage("create experiment catalog schema", path, &error))?;
            }
            Ok::<_, CoreError>(())
        })?;
        match catalog.value("experiment")? {
            Some(held) if held == experiment => Ok(catalog),
            Some(held) => Err(CoreError::StateConflict {
                operation: "open experiment catalog",
                detail: format!(
                    "{} catalogs experiment {held}, not {experiment}",
                    path.display()
                ),
            }),
            None => {
                block_on_inline(
                    catalog.transaction("record experiment", async |connection| {
                        for (key, value) in [("experiment", experiment), ("design", design)] {
                            connection
                                .execute_with_params(
                                    "INSERT INTO catalog (key, value) VALUES (?1, ?2)",
                                    &[text(key), text(value)],
                                )
                                .await
                                .map_err(|error| storage("record experiment", path, &error))?;
                        }
                        Ok(())
                    }),
                )?;
                Ok(catalog)
            }
        }
    }

    /// The database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The design text the catalog was created with.
    ///
    /// # Errors
    /// [`CoreError::StorageFailed`] when it cannot be read.
    pub fn design(&self) -> Result<Option<String>, CoreError> {
        self.value("design")
    }

    fn value(&self, key: &str) -> Result<Option<String>, CoreError> {
        let rows = block_on_inline(
            self.connection
                .query_with_params("SELECT value FROM catalog WHERE key = ?1", &[text(key)]),
        )
        .map_err(|error| storage("read experiment catalog", &self.path, &error))?;
        rows.first().map(|row| string(row, 0)).transpose()
    }

    /// Every cataloged run, in run order.
    ///
    /// # Errors
    /// [`CoreError::StorageFailed`] when they cannot be read;
    /// [`CoreError::StateConflict`] for a malformed row.
    pub fn runs(&self) -> Result<Vec<CatalogedRun>, CoreError> {
        let rows = block_on_inline(self.connection.query(
            "SELECT ordinal, condition, split, seed, config_hash, status, result, manifest, \
             records_sha256 FROM runs ORDER BY ordinal",
        ))
        .map_err(|error| storage("read experiment runs", &self.path, &error))?;
        rows.iter()
            .map(|row| {
                Ok(CatalogedRun {
                    ordinal: integer(row, 0)?,
                    condition: string(row, 1)?,
                    split: string(row, 2)?,
                    seed: integer(row, 3)?,
                    config_hash: string(row, 4)?,
                    status: string(row, 5)?,
                    result: string(row, 6)?,
                    manifest: string(row, 7)?,
                    records_sha256: string(row, 8)?,
                })
            })
            .collect()
    }

    /// Catalog `run`, durably. Cataloging the same run again with the same
    /// row is a no-op.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when its ordinal or its `(condition,
    /// split, seed)` is already cataloged with another row;
    /// [`CoreError::StorageFailed`] when it cannot be written.
    pub fn record(&mut self, run: &CatalogedRun) -> Result<(), CoreError> {
        let path = self.path.clone();
        let held = self.runs()?;
        if let Some(existing) = held.iter().find(|existing| {
            existing.ordinal == run.ordinal
                || (existing.condition == run.condition
                    && existing.split == run.split
                    && existing.seed == run.seed)
        }) {
            if existing == run {
                return Ok(());
            }
            return Err(CoreError::StateConflict {
                operation: "catalog run",
                detail: format!(
                    "run {} ({} {} seed {}) is already cataloged as run {} ({} {} seed {})",
                    run.ordinal,
                    run.condition,
                    run.split,
                    run.seed,
                    existing.ordinal,
                    existing.condition,
                    existing.split,
                    existing.seed
                ),
            });
        }
        block_on_inline(self.transaction("catalog run", async |connection| {
            connection
                .execute_with_params(
                    "INSERT INTO runs (ordinal, condition, split, seed, config_hash, status, \
                     result, manifest, records_sha256) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    &[
                        int(run.ordinal),
                        text(&run.condition),
                        text(&run.split),
                        int(run.seed),
                        text(&run.config_hash),
                        text(&run.status),
                        text(&run.result),
                        text(&run.manifest),
                        text(&run.records_sha256),
                    ],
                )
                .await
                .map_err(|error| storage("catalog run", &path, &error))?;
            Ok(())
        }))
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
}
