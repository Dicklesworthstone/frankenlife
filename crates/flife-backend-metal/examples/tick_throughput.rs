//! Whole-tick throughput of the reference small-world preset, or of the
//! configuration a sixth argument names, under each `--controllers` choice,
//! the worlds stepped in rotating blocks in one process (the reference live
//! in the same invocation), every world's digest compared with the first's
//! after every block.
//!
//! `cargo run --release -p flife-backend-metal --example tick_throughput -- [workers] [ticks per block] [blocks] [warm-up ticks] [choices] [config]`,
//! for example `-- 1 20 8 40 cpu,torch,metal`. A choice may add phases on
//! Metal after a `+`: `cpu+sensors` samples phase 3 on `MetalSensors`, and
//! `cpu+field` steps phase 2 on `MetalField`, and `cpu+uptake` computes
//! phase 5's demands and grants on `MetalUptake` (FL-031), for example
//! `cpu,cpu+sensors,metal+sensors+field+uptake`; `+settle` runs phases 6 to 8
//! on `MetalSettle`, and `+all` puts the field, sensors, uptake and settlement
//! on one device owner (FL-032). `+slots=N` puts the controllers on
//! a `MetalBackend` with `N` genomes to a parameter shard (FL-026), for
//! example `metal,metal+slots=4096`. A choice this host refuses
//! (`metal` without a device) is reported and left out. Prints one JSON line.

use std::num::NonZeroUsize;
use std::time::Instant;

use flife_backend_metal::{
    BackendBudget, ControllerChoice, MetalBackend, MetalField, MetalSensors, MetalSettle,
    MetalUptake,
};
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

fn found(workers: NonZeroUsize, name: &str, config: &ValidatedConfig) -> Result<World, CoreError> {
    let refused = |detail: String| CoreError::StateConflict {
        operation: "tick_throughput",
        detail,
    };
    let mut parts = name.split('+');
    let choice: ControllerChoice = parts.next().unwrap_or_default().parse().map_err(refused)?;
    let mut world = World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32()?,
    )?;
    world.set_cpu_workers(workers)?;
    world.set_controller_backend(choice.backend(workers.get(), BackendBudget::UNBOUNDED)?);
    for phase in parts {
        match phase {
            "sensors" => world.set_sensor_backend(Some(Box::new(MetalSensors::open()?))),
            "field" => world.set_field_backend(Some(Box::new(MetalField::open()?))),
            "uptake" => world.set_uptake_backend(Some(Box::new(MetalUptake::open()?))),
            "settle" => world.set_settle_backend(Some(Box::new(MetalSettle::open()?))),
            "all" => {
                let settle = MetalSettle::open()?;
                let owner = settle.owner().clone();
                world.set_field_backend(Some(Box::new(MetalField::with_owner(owner.clone())?)));
                world.set_sensor_backend(Some(Box::new(MetalSensors::with_owner(owner.clone())?)));
                world.set_uptake_backend(Some(Box::new(MetalUptake::with_owner(owner)?)));
                world.set_settle_backend(Some(Box::new(settle)));
            }
            other => {
                let slots: u64 = other
                    .strip_prefix("slots=")
                    .and_then(|slots| slots.parse().ok())
                    .ok_or_else(|| {
                        refused(format!(
                            "`{other}` is not `sensors`, `field`, `uptake`, `settle`, `all` or \
                             `slots=N`"
                        ))
                    })?;
                world
                    .set_controller_backend(Some(Box::new(MetalBackend::with_shard_slots(slots)?)));
            }
        }
    }
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
    let config = match std::env::args().nth(6) {
        Some(path) => {
            let text =
                std::fs::read_to_string(&path).map_err(|error| CoreError::StateConflict {
                    operation: "tick_throughput",
                    detail: format!("read {path}: {error}"),
                })?;
            ValidatedConfig::from_toml(&text)?
        }
        None => ValidatedConfig::from_toml(PRESET)?,
    };

    let mut entries = Vec::new();
    for name in choices.split(',') {
        match found(workers, name, &config) {
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
            let (sensors, field) = (entry.world.sensor_report(), entry.world.field_report());
            let uptake = entry.world.uptake_report();
            let settle = entry.world.settle_report();
            let rate = median(&entry.rates);
            format!(
                "{{\"choice\":\"{}\",\"ticks_per_s\":{rate:.3},\"cv_percent\":{:.2},\
                 \"ratio_to_first\":{:.3},\"samples\":{:?},\"batched\":{},\"deferred\":{},\
                 \"reference_ticks\":{},\"sensor_ticks\":[{},{}],\"field_steps\":[{},{}],\
                 \"uptake_ticks\":[{},{}],\"field_reductions\":[{},{}],\
                 \"field_checks\":[{},{}],\"settle_ticks\":[{},{}]}}",
                entry.name,
                cv(&entry.rates),
                rate / baseline,
                entry.rates,
                report.batched,
                report.deferred,
                report.reference_ticks,
                sensors.ticks,
                sensors.reference_ticks,
                field.steps,
                field.reference_steps,
                uptake.ticks,
                uptake.reference_ticks,
                field.reductions,
                field.reference_reductions,
                field.checks,
                field.reference_checks,
                settle.ticks,
                settle.reference_ticks
            )
        })
        .collect();
    let digest = entries
        .first()
        .map_or_else(String::new, |entry| entry.world.digest().to_hex());
    println!(
        "{{\"workers\":{workers},\"ticks_per_block\":{ticks},\"blocks\":{blocks},\"warm_up\":{warm},\
         \"live\":{live},\"digests_equal\":{equal},\"digest\":\"{digest}\",\"choices\":[{}]}}",
        rows.join(",")
    );
    Ok(())
}
