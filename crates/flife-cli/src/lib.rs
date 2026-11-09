//! FrankenLife's headless command line (FL-015; plan §14.6, §15.1, §15.2).
//!
//! - [`validate`]: a validated configuration's canonical summary.
//! - [`mod@run`]: a headless run through the FL-014 session owner on
//!   Asupersync's native runtime, emitting versioned NDJSON [`Record`]s.
//! - [`observe`]: what a run did. A [`Tally`] of the published reports, and
//!   a [`Survey`] of a committed world's raw state: generations, lineages,
//!   distinct genomes and position-by-position heredity between live
//!   relatives.
//!
//! The `flife` binary wraps these as `flife validate <config>` and
//! `flife run <config> --ticks N --every K`. The repository's presets live
//! in `presets/`.

#![forbid(unsafe_code)]

pub mod observe;
pub mod run;
pub mod validate;

pub use observe::{Heredity, Survey, Tally};
pub use run::{Finished, RUN_SCHEMA, RUN_SESSION, Record, RunOptions, Stop, found, run, start};
pub use validate::{VALIDATE_SCHEMA, Validation};

/// The schema of an error record.
pub const ERROR_SCHEMA: &str = "frankenlife.error.v1";
