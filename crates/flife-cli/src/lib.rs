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
//! - [`checkpointing`]: how a run directory's checkpoints are taken
//!   (FL-038): `pause-and-stream`, streamed into the chunk store a page at
//!   a time while the run waits, or `bounded-background`, drained by a
//!   writer through a queue bounded by `checkpoint_retained_bytes` while
//!   the run steps on; and [`checkpointing::admit`], their memory
//!   admission.
//! - [`metrics`]: incremental metrics and lineage summaries (FL-040). A
//!   run with `metrics_every` folds every report into a [`Tracker`] and
//!   emits a [`Metrics`] record every so many ticks: complete window,
//!   population and lineage statistics, and statistics sampled from the
//!   render epoch at the record's tick.
//! - [`mod@experiment`]: seed ensembles, sweeps and ablations (FL-041).
//!   [`experiment()`] runs an [`Experiment`]'s conditions over discovery
//!   and held-out seeds into an experiment directory, catalogs every run
//!   with its manifest whatever its ending, continues after a kill, and
//!   writes a [`Report`] of paired comparisons with the control that
//!   depends only on the catalog.
//! - [`mod@discovery`]: saved discoveries (FL-042). [`discover()`] saves a
//!   run checkpoint's discovery (selection, thumbnail, presentation,
//!   description) into a bundle directory with a full-world export, and
//!   pins the checkpoint in the run's store against retention.
//! - [`mod@inspect`]: a run directory's committed state at one checkpoint,
//!   organism by organism and optionally cell by cell (plan §14.6).
//! - [`observe`]: what a run did. A [`Tally`] of the published reports, and
//!   a [`Survey`] of a committed world's raw state: generations, lineages,
//!   distinct genomes and position-by-position heredity between live
//!   relatives.
//!
//! The `flife` binary wraps these as `flife validate <config>`,
//! `flife run <config> --ticks N --every K [--run-dir DIR]`,
//! `flife resume DIR --until T`,
//! `flife branch PARENT DIR [--at T] [--name N] [--inject …] [--remove …]`,
//! `flife inspect DIR [--at T] [--field]`,
//! `flife discover DIR BUNDLE [--at T] [--region X,Y,R | --lineage L]` and
//! `flife experiment SPEC --out DIR`. The repository's presets live
//! in `presets/`.

#![forbid(unsafe_code)]

pub mod branch;
pub mod checkpointing;
pub mod discovery;
pub mod experiment;
pub mod inspect;
pub mod memory;
pub mod metrics;
pub mod observe;
pub mod persist;
pub mod render;
pub mod run;
pub mod validate;

pub use branch::{
    BRANCH_NAME_MAX_BYTES, BranchOptions, BranchParent, ForkRecord, FoundingChunks, Inherited,
    branch, branch_session,
};
pub use checkpointing::{
    BOUNDED_BACKGROUND, CHECKPOINT_FIXED_BYTES, CheckpointAdmission, CheckpointMode,
    PAUSE_AND_STREAM, admit,
};
pub use discovery::{
    Camera, DESCRIPTION_MAX_BYTES, DISCOVERY_FILE, DISCOVERY_SCHEMA, DiscoverOptions, Discovery,
    Disk, Exports, FullWorld, Presentation, REGION_FILL, Selected, Selection, Source,
    THUMBNAIL_FILE, THUMBNAIL_MAX_PIXELS, Thumbnail, WORLD_DIR, discover, discovery_id,
};
pub use experiment::{
    ABLATIONS, Ablation, Condition, EXPERIMENT_CATALOG_FILE, EXPERIMENT_SCHEMA, Experiment,
    ExperimentOptions, ExperimentRecord, LastRecord, MANIFEST_SCHEMA, MEASURES, Measure, Measures,
    REPORT_FILE, REPORT_SCHEMA, Report, RunResult, Spec, experiment,
};
pub use flife_backend_metal::{BackendBudget, ControllerChoice};
pub use flife_manim::{ColorMapping, FieldComponent, FieldMapping, Look, Particles, Style};
pub use inspect::{
    INSPECT_SCHEMA, InspectOptions, Inspected, InspectedCheckpoint, InspectedExtent,
    InspectedField, InspectedLayer, InspectedOrganism, InspectedWeights, Inspection, inspect,
};
pub use memory::{MemoryAdmission, RESERVE_BYTES};
pub use metrics::{Declaration, METRICS_SCHEMA, Metrics, Tracked, Tracker};
pub use observe::{Heredity, Survey, Tally};
pub use persist::{
    CATALOG_FILE, CONFIG_FILE, Checkpoints, Injection, Introduction, Material, Persistence,
    Recovered, Removal, ResumeOptions, STORE_DIR, ScheduledCommand, StoreDurability, create,
    open_session, recover, resume, vacant,
};
pub use render::{
    ENCODERS, ENCODING_BYTES_FIXED, ENCODING_BYTES_PER_PIXEL, RENDER_SCHEMA, RenderOptions,
    RenderProjection, RenderRecord, encoding_bytes, render,
};
pub use run::{
    AUTO_WORKER_CEILING, CheckpointAt, Finished, JournalMarks, Persisted, RUN_SCHEMA, RUN_SESSION,
    Record, RunOptions, SkippedCheckpoint, Stop, auto_workers, configure, found, run, run_with,
    start,
};
pub use validate::{VALIDATE_SCHEMA, Validation};

/// The schema of an error record.
pub const ERROR_SCHEMA: &str = "frankenlife.error.v1";
