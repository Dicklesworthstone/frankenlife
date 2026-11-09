//! `flife`: FrankenLife's headless command line (FL-015; plan §14.6).
//!
//! Every line on standard output is one versioned JSON record. An error is
//! one `frankenlife.error.v1` record on standard error. Exit status: 0 when
//! the command did what it was asked, 1 when a run stopped early (a
//! capacity pause or a failed tick), 2 for an unreadable or invalid
//! configuration or a refused session.

#![forbid(unsafe_code)]

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use flife_cli::{ERROR_SCHEMA, RunOptions, Stop, Validation};
use flife_core::ValidatedConfig;
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(name = "flife", version, about = "FrankenLife's headless command line")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate a configuration and print its canonical summary.
    Validate {
        /// The configuration or preset, as TOML.
        config: PathBuf,
    },
    /// Run a configuration headlessly through the session owner, printing
    /// NDJSON records.
    Run {
        /// The configuration or preset, as TOML.
        config: PathBuf,
        /// Ticks to publish.
        #[arg(long, default_value_t = 1024)]
        ticks: u64,
        /// Print a progress record every this many ticks; 0 for none.
        #[arg(long, default_value_t = 128)]
        every: u64,
    },
}

#[derive(Serialize)]
struct ErrorRecord<'a> {
    schema: &'static str,
    kind: &'a str,
    detail: String,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Validate { config } => match load(&config) {
            Ok(config) => {
                print(&Validation::of(&config));
                ExitCode::SUCCESS
            }
            Err(code) => code,
        },
        Command::Run {
            config,
            ticks,
            every,
        } => {
            let config = match load(&config) {
                Ok(config) => config,
                Err(code) => return code,
            };
            match flife_cli::run(&config, RunOptions { ticks, every }, print) {
                Ok(finished) if finished.stop == Stop::Reached => ExitCode::SUCCESS,
                Ok(_) => ExitCode::from(1),
                Err(error) => fail(error.kind(), error.to_string()),
            }
        }
    }
}

fn load(path: &PathBuf) -> Result<ValidatedConfig, ExitCode> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| fail("io", format!("{}: {error}", path.display())))?;
    ValidatedConfig::from_toml(&text).map_err(|error| fail(error.kind(), error.to_string()))
}

fn print(record: &impl Serialize) {
    match serde_json::to_string(record) {
        Ok(line) => {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
        Err(error) => {
            fail("serialization", error.to_string());
        }
    }
}

fn fail(kind: &str, detail: String) -> ExitCode {
    let record = ErrorRecord {
        schema: ERROR_SCHEMA,
        kind,
        detail,
    };
    if let Ok(line) = serde_json::to_string(&record) {
        let _ = writeln!(std::io::stderr().lock(), "{line}");
    }
    ExitCode::from(2)
}
