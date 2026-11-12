//! `flife inspect` (plan §14.6; FL-048's raw state): a run directory's
//! committed world at one checkpoint, organism by organism and cell by
//! cell, and on demand an organism's whole controller (plan §14.2). The
//! oracle is a world founded and stepped directly in this process, never
//! the checkpoint the command restores.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use flife_core::ValidatedConfig;
use flife_engine::{Stepped, World};
use serde_json::Value;

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// The 32 × 32 ecology of 48 founders the resume tests use, volatile.
fn ecology_text() -> String {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    for (section, key, value) in [
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "1024"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "120.0"),
        ("sources", "sigma_world_units", "3.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.6"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.2",
        ),
        ("reproduction", "maximum_attempt_rate_per_second", "4.0"),
        ("reproduction", "cooldown_seconds", "0.25"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ] {
        let header = format!("[{section}]");
        let first = lines
            .iter()
            .position(|line| line.trim() == header)
            .expect("a section");
        let line = lines[first + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| first + 1 + offset)
            .expect("a key");
        lines[line] = format!("{key} = {value}");
    }
    lines.join("\n")
}

fn scratch() -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("flife-inspect");
    fs::create_dir_all(&path).expect("creates the scratch directory");
    path
}

/// A path that does not exist yet.
fn fresh(name: &str) -> PathBuf {
    let path = scratch().join(name);
    if path.exists() {
        fs::remove_dir_all(&path).expect("clears the last run's directory");
    }
    path
}

/// The configuration, written through a uniquely named temporary file
/// renamed into place.
fn config_file() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = scratch().join("ecology.toml");
    let temporary = path.with_extension(format!(
        "{}.{}.partial",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&temporary, ecology_text()).expect("writes the configuration");
    fs::rename(&temporary, &path).expect("renames it into place");
    path
}

fn text(path: &Path) -> &str {
    path.to_str().expect("UTF-8")
}

fn flife(args: &[&str]) -> (Option<i32>, Vec<Value>, Vec<Value>) {
    let output = Command::new(env!("CARGO_BIN_EXE_flife"))
        .args(args)
        .output()
        .expect("flife runs");
    let lines = |bytes: &[u8]| -> Vec<Value> {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(|line| serde_json::from_str(line).expect("each line is one JSON record"))
            .collect()
    };
    (
        output.status.code(),
        lines(&output.stdout),
        lines(&output.stderr),
    )
}

/// The world of [`ecology_text`] stepped directly to `tick`.
fn direct(tick: u64) -> World {
    let config = ValidatedConfig::from_toml(&ecology_text()).expect("a valid configuration");
    let mut world = flife_cli::found(&config).expect("the world founds");
    while world.tick().get() < tick {
        match world.step().expect("the tick advances") {
            Stepped::Published(_) => {}
            Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
        }
    }
    world
}

/// A run to tick 30 checkpointed every 7 ticks keeps 14, 21, 28 and 30 (the
/// configuration retains four). Inspecting it reports the newest, or the
/// one asked for, exactly as a world stepped directly to that tick holds
/// it: every organism's identities, ancestry, position, velocity, energy,
/// topology, parameter count and affinity bit for bit, in identity order,
/// and with `--field` every cell's energy. A retired or never-taken tick
/// is refused.
#[test]
fn inspection_reports_the_committed_world_as_a_direct_world_holds_it() {
    let config = config_file();
    let run_dir = fresh("run");
    let (status, out, err) = flife(&[
        "run",
        text(&config),
        "--ticks",
        "30",
        "--every",
        "0",
        "--run-dir",
        text(&run_dir),
        "--checkpoint-every",
        "7",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let end = out
        .iter()
        .find(|record| record["record"] == "end")
        .expect("an end record");

    let (status, newest, err) = flife(&["inspect", text(&run_dir)]);
    assert_eq!(status, Some(0), "{err:?}");
    let header = &newest[0];
    assert_eq!(header["record"], "inspect");
    assert_eq!(header["schema"], flife_cli::INSPECT_SCHEMA);
    assert_eq!(header["checkpoint"]["tick"], 30);
    assert_eq!(header["digest"], end["digest"], "the run's last world");

    for tick in [28_u64, 21] {
        let (status, records, err) = flife(&[
            "inspect",
            text(&run_dir),
            "--at",
            &tick.to_string(),
            "--field",
        ]);
        assert_eq!(status, Some(0), "{err:?}");
        let world = direct(tick);
        let header = &records[0];
        assert_eq!(header["checkpoint"]["tick"], tick);
        assert_eq!(header["digest"], world.digest().to_hex());
        assert_eq!(header["model"], world.config().config().model);
        let grid = world.field().grid();
        assert_eq!(header["world"]["width"], grid.world_width());
        assert_eq!(header["world"]["cells"][0], grid.cells_x());
        let organisms: Vec<&Value> = records
            .iter()
            .filter(|record| record["record"] == "organism")
            .collect();
        let population = world.population();
        assert_eq!(header["live"], organisms.len());
        assert_eq!(organisms.len(), population.iter().count());
        let mut energy = 0.0;
        for (record, organism) in organisms.iter().zip(population.iter()) {
            let genome = population.genome(organism.id).expect("a genome");
            let float = |value: &Value| value.as_f64().expect("a number").to_bits();
            assert_eq!(record["id"], organism.id.get());
            assert_eq!(record["genome"], organism.genome.get());
            assert_eq!(record["lineage"], organism.lineage.get());
            assert_eq!(record["generation"], organism.generation.get());
            assert_eq!(
                record["parent"].as_u64(),
                organism.parent.map(|parent| parent.get())
            );
            assert_eq!(record["birth_tick"], organism.birth_tick.get());
            for axis in 0..2 {
                assert_eq!(
                    float(&record["position"][axis]),
                    organism.position[axis].to_bits()
                );
                assert_eq!(
                    float(&record["velocity"][axis]),
                    organism.velocity[axis].to_bits()
                );
            }
            assert_eq!(float(&record["energy"]), organism.energy.to_bits());
            assert_eq!(record["topology"], genome.topology().index());
            assert_eq!(record["parameters"], genome.parameters().len());
            let affinity: Vec<u64> = record["affinity"]
                .as_array()
                .expect("a list")
                .iter()
                .map(float)
                .collect();
            let expected: Vec<u64> = genome
                .traits()
                .affinity()
                .iter()
                .map(|value| value.to_bits())
                .collect();
            assert_eq!(affinity, expected);
            energy += organism.energy;
        }
        assert_eq!(
            header["organism_energy"].as_f64().expect("a number"),
            energy
        );
        let field = records.last().expect("records");
        assert_eq!(field["record"], "field");
        let cells: Vec<u32> = field["energy"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|value| {
                // Each cell is an f32 widened exactly, so it narrows back.
                #[expect(clippy::cast_possible_truncation, reason = "exact by construction")]
                let cell = value.as_f64().expect("a number") as f32;
                cell.to_bits()
            })
            .collect();
        let expected: Vec<u32> = world
            .field()
            .energy()
            .iter()
            .map(|cell| cell.to_bits())
            .collect();
        assert_eq!(cells, expected, "every cell, row-major");
        assert_eq!(
            header["field_energy"].as_f64().expect("a number").to_bits(),
            world.field().energy_total().to_bits()
        );
    }

    for tick in ["7", "29"] {
        let (status, out, err) = flife(&["inspect", text(&run_dir), "--at", tick]);
        assert_eq!(status, Some(2), "{out:?}");
        assert_eq!(err[0]["kind"], "state-conflict", "{err:?}");
    }
}

/// A directory that holds no run is refused and left as it was: no store
/// is created in it.
#[test]
fn a_directory_without_a_run_is_refused_untouched() {
    let empty = fresh("empty");
    fs::create_dir_all(&empty).expect("creates it");
    let (status, out, err) = flife(&["inspect", text(&empty)]);
    assert_ne!(status, Some(0), "{out:?}");
    assert!(!err.is_empty());
    assert_eq!(fs::read_dir(&empty).expect("lists").count(), 0);
}

/// Plan §14.2's on-demand weight dump: `--weights` prints each organism it
/// names after every other record, in the order named: its identity,
/// genome, topology and parameter count, and its layers (104 inputs into
/// the first, each fan-in the last fan-out, 36 outputs from the last), whose
/// rows of weights then biases, read back as `f32`, are its genome's
/// parameters in a world stepped directly to that tick, bit for bit. An
/// organism the checkpoint's world does not hold is refused.
#[test]
fn a_weight_dump_prints_an_organisms_whole_controller() {
    let config = config_file();
    let run_dir = fresh("weights");
    let (status, _, err) = flife(&[
        "run",
        text(&config),
        "--ticks",
        "30",
        "--every",
        "0",
        "--run-dir",
        text(&run_dir),
        "--checkpoint-every",
        "7",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let world = direct(28);
    let population = world.population();
    let ids: Vec<_> = population.ids().collect();
    let named = [*ids.last().expect("organisms"), ids[0]];
    let (status, records, err) = flife(&[
        "inspect",
        text(&run_dir),
        "--at",
        "28",
        "--weights",
        &named[0].get().to_string(),
        "--weights",
        &named[1].get().to_string(),
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(
        records.len(),
        1 + ids.len() + 2,
        "header, organisms, two dumps"
    );
    let narrow = |value: &Value| {
        // Each parameter is an f32 written out exactly, so it reads back.
        #[expect(clippy::cast_possible_truncation, reason = "exact by construction")]
        let parameter = value.as_f64().expect("a number") as f32;
        parameter.to_bits()
    };
    for (dump, id) in records[1 + ids.len()..].iter().zip(named) {
        let genome = population.genome(id).expect("its genome");
        assert_eq!(dump["record"], "weights");
        assert_eq!(dump["id"], id.get());
        assert_eq!(
            dump["genome"],
            population.get(id).expect("lives").genome.get()
        );
        assert_eq!(dump["topology"], genome.topology().index());
        assert_eq!(dump["parameters"], genome.parameters().len());
        let mut fan_in = 104;
        let mut read = Vec::new();
        for layer in dump["layers"].as_array().expect("layers") {
            assert_eq!(layer["inputs"], fan_in);
            let outputs =
                usize::try_from(layer["outputs"].as_u64().expect("a width")).expect("a width");
            let rows = layer["weights"].as_array().expect("rows");
            assert_eq!(rows.len(), outputs);
            for row in rows {
                let row = row.as_array().expect("a row");
                assert_eq!(row.len(), fan_in);
                read.extend(row.iter().map(narrow));
            }
            let biases = layer["biases"].as_array().expect("biases");
            assert_eq!(biases.len(), outputs);
            read.extend(biases.iter().map(narrow));
            fan_in = outputs;
        }
        assert_eq!(fan_in, 36, "the raw outputs last");
        let stored: Vec<u32> = genome
            .parameters()
            .iter()
            .map(|parameter| parameter.to_bits())
            .collect();
        assert_eq!(read, stored, "every parameter, in the controller's layout");
    }

    let (status, out, err) = flife(&[
        "inspect",
        text(&run_dir),
        "--at",
        "28",
        "--weights",
        &(1_u64 << 40).to_string(),
    ]);
    assert_eq!(status, Some(2), "{out:?}");
    assert_eq!(err[0]["kind"], "state-conflict", "{err:?}");
}
