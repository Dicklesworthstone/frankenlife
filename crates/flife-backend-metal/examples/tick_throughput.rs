//! Whole-tick throughput of the reference small-world preset under each
//! `--controllers` choice, the worlds stepped in rotating blocks in one
//! process (the reference live in the same invocation), every world's digest
//! compared with the first's after every block.
//!
//! `cargo run --release -p flife-backend-metal --example tick_throughput -- [workers] [ticks per block] [blocks] [warm-up ticks] [choices]`,
//! for example `-- 1 20 8 40 cpu,torch,metal`. A choice this host refuses
//! (`metal` without a device) is reported and left out. Prints one JSON line.

use std::num::NonZeroUsize;
use std::time::Instant;

use flife_backend_metal::ControllerChoice;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

fn found(workers: NonZeroUsize, choice: ControllerChoice) -> Result<World, CoreError> {
    let config = ValidatedConfig::from_toml(PRESET)?;
    let mut world = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32()?,
    )?;
    world.set_cpu_workers(workers)?;
    world.set_controller_backend(choice.backend(workers.get())?);
    Ok(world)
}

/// Step `ticks` published ticks; the seconds taken and the last live count.
fn block(world: &mut World, ticks: u64) -> Result<(f64, usize), CoreError> {
    let start = Instant::now();
    let mut live = 0;
    for _ in 0..ticks {
        match world.step()? {
            Stepped::Published(report) => live = report.live,
            other => {
                return Err(CoreError::StateConflict {
                    operation: "tick_throughput",
                    detail: format!("{other:?}"),
                });
            }
        }
    }
    Ok((start.elapsed().as_secs_f64(), live))
}

fn median(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted.get(sorted.len() / 2).copied().unwrap_or(f64::NAN)
}

/// The coefficient of variation, in percent.
fn cv(samples: &[f64]) -> f64 {
    let n = samples.len() as f64;
    let mean = samples.iter().sum::<f64>() / n;
    let variance = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    100.0 * variance.sqrt() / mean
}

struct Entry {
    name: String,
    world: World,
    rates: Vec<f64>,
}

fn main() -> Result<(), CoreError> {
    let mut args = std::env::args().skip(1);
    let mut next = |default: usize| -> usize {
        args.next()
            .and_then(|arg| arg.parse().ok())
            .unwrap_or(default)
    };
    let workers = NonZeroUsize::new(next(1)).unwrap_or(NonZeroUsize::MIN);
    let ticks = next(20) as u64;
    let blocks = next(8);
    let warm = next(40) as u64;
    let choices = std::env::args()
        .nth(5)
        .unwrap_or_else(|| "cpu,torch,metal".to_owned());

    let mut entries = Vec::new();
    for name in choices.split(',') {
        let choice: ControllerChoice = name.parse().map_err(|detail| CoreError::StateConflict {
            operation: "tick_throughput",
            detail,
        })?;
        match found(workers, choice) {
            Ok(mut world) => {
                block(&mut world, warm)?;
                entries.push(Entry {
                    name: name.to_owned(),
                    world,
                    rates: Vec::new(),
                });
            }
            Err(error) => eprintln!("{name}: left out: {error}"),
        }
    }
    let mut equal = true;
    let mut live = 0;
    for index in 0..blocks {
        // Rotate which world goes first, so none always runs warm.
        let count = entries.len();
        for offset in 0..count {
            let Some(entry) = entries.get_mut((index + offset) % count) else {
                continue;
            };
            let (seconds, now) = block(&mut entry.world, ticks)?;
            entry.rates.push(ticks as f64 / seconds);
            live = now;
        }
        if let Some((first, rest)) = entries.split_first() {
            let digest = first.world.digest();
            equal &= rest.iter().all(|entry| entry.world.digest() == digest);
        }
    }
    let baseline = entries
        .first()
        .map_or(f64::NAN, |entry| median(&entry.rates));
    let rows: Vec<String> = entries
        .iter()
        .map(|entry| {
            let report = entry.world.controller_report();
            let rate = median(&entry.rates);
            format!(
                "{{\"choice\":\"{}\",\"ticks_per_s\":{rate:.3},\"cv_percent\":{:.2},\
                 \"ratio_to_first\":{:.3},\"samples\":{:?},\"batched\":{},\"deferred\":{},\
                 \"reference_ticks\":{}}}",
                entry.name,
                cv(&entry.rates),
                rate / baseline,
                entry.rates,
                report.batched,
                report.deferred,
                report.reference_ticks
            )
        })
        .collect();
    println!(
        "{{\"workers\":{workers},\"ticks_per_block\":{ticks},\"blocks\":{blocks},\"warm_up\":{warm},\
         \"live\":{live},\"digests_equal\":{equal},\"choices\":[{}]}}",
        rows.join(",")
    );
    Ok(())
}
