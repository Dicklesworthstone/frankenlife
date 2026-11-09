//! FrankenLife's headless command line (FL-015; plan §14.6, §15.1, §15.2).
//!
//! - [`validate`]: a validated configuration's canonical summary.
//! - [`mod@run`]: a headless run through the FL-014 session owner on
//!   Asupersync's native runtime, emitting versioned NDJSON [`Record`]s.
//! - [`persist`]: run directories (FL-037). A run with one keeps its
//!   configuration, its FL-036 command journal and its FL-035 checkpoints
//!   there, and [`resume`] continues it from the newest checkpoint that
//!   restores, reaching exactly the uninterrupted run's state.
//! - [`mod@branch`]: branches (FL-039). [`branch()`] starts a new run
//!   directory from a checkpoint of another, as a new session that
//!   inherits its commands and shares its stored chunks; each branch then
//!   resumes on its own.
//! - [`observe`]: what a run did. A [`Tally`] of the published reports, and
//!   a [`Survey`] of a committed world's raw state: generations, lineages,
//!   distinct genomes and position-by-position heredity between live
//!   relatives.
//!
//! The `flife` binary wraps these as `flife validate <config>`,
//! `flife run <config> --ticks N --every K [--run-dir DIR]`,
//! `flife resume DIR --until T` and
//! `flife branch PARENT DIR [--at T] [--name N] [--inject …]`. The repository's presets live
//! in `presets/`.

#![forbid(unsafe_code)]

pub mod branch;
pub mod observe;
pub mod persist;
pub mod render;
pub mod run;
pub mod validate;

pub use branch::{
    BRANCH_NAME_MAX_BYTES, BranchOptions, BranchParent, ForkRecord, FoundingChunks, Inherited,
    branch, branch_session,
};
pub use flife_backend_metal::ControllerChoice;
pub use flife_manim::{ColorMapping, Look, Particles, Style};
pub use observe::{Heredity, Survey, Tally};
pub use persist::{
    CATALOG_FILE, CONFIG_FILE, Injection, Persistence, ResumeOptions, STORE_DIR, StoreDurability,
    resume,
};
pub use render::{RENDER_SCHEMA, RenderOptions, RenderRecord, render};
pub use run::{
    AUTO_WORKER_CEILING, CheckpointAt, Finished, JournalMarks, Persisted, RUN_SCHEMA, RUN_SESSION,
    Record, RunOptions, SkippedCheckpoint, Stop, auto_workers, configure, found, run, run_with,
    start,
};
pub use validate::{VALIDATE_SCHEMA, Validation};

/// The schema of an error record.
pub const ERROR_SCHEMA: &str = "frankenlife.error.v1";
