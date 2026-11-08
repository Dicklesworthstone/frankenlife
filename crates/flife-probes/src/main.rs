//! `flife-probe`: run the FL-001 capability probes and print one NDJSON line
//! per probe (schema `flife.probe.v1`) on stdout.
//!
//! ```text
//! flife-probe [--out-dir DIR] [--window [POLLS]]
//! ```
//!
//! `--out-dir` receives the Manim probe's PNGs (default: the system temp
//! directory). `--window` also opens a real native preview window on the main
//! thread and polls it (default 30 polls). That probe is opt-in because it
//! shows a window.
//!
//! Exit codes: 0 when no probe failed (qualified, unavailable and gap verdicts
//! are all honest outcomes), 1 when any probe failed, 2 for a usage error.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

struct Args {
    out_dir: PathBuf,
    window_polls: Option<u32>,
}

fn parse_args() -> Result<Args, String> {
    let mut out_dir = std::env::temp_dir();
    let mut window_polls = None;
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out-dir" => {
                out_dir = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or("--out-dir needs a directory")?;
            }
            "--window" => {
                let polls = match args.peek().map(|next| next.parse::<u32>()) {
                    Some(Ok(polls)) => {
                        args.next();
                        polls
                    }
                    _ => 30,
                };
                window_polls = Some(polls);
            }
            "-h" | "--help" => {
                return Err("usage: flife-probe [--out-dir DIR] [--window [POLLS]]".into());
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    if !out_dir.is_dir() {
        return Err(format!(
            "--out-dir {} is not a directory",
            out_dir.display()
        ));
    }
    Ok(Args {
        out_dir,
        window_polls,
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("flife-probe: {message}");
            return ExitCode::from(2);
        }
    };
    let mut reports = flife_probes::headless_probes(&args.out_dir, "cli");
    if let Some(polls) = args.window_polls {
        reports.push(flife_probes::native_window_probe(polls));
    }
    for report in &reports {
        println!("{}", report.to_json_line());
    }
    if reports.iter().any(flife_probes::ProbeReport::failed) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
