//! FL-015 evidence (plan §7.2, §15.1, §15.2, §19.5, §22.2, Appendix A): the
//! real small-world preset and its smoke run.
//!
//! `presets/reference-small-world.toml` is Appendix A at the Reference
//! profile's scale with a declared viability tuning. The smoke run drives
//! that preset through the FL-014 session owner on Asupersync's native
//! runtime for 1,024 ticks (8 simulated seconds). Births, deaths and
//! emissions are read from the published reports and cross-checked against
//! the committed world's raw state. Heritable variation is read from the
//! raw genomes alone: live parents, children and grandchildren are compared
//! parameter by parameter and in trait space, against unrelated founders.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

use flife_cli::{
    ControllerChoice, ERROR_SCHEMA, Finished, Heredity, RUN_SCHEMA, RUN_SESSION, Record,
    RunOptions, Stop, Survey, Tally, VALIDATE_SCHEMA, Validation, run,
};
use flife_core::{CoreError, ValidatedConfig};
use flife_runtime::SessionConfig;
use serde_json::Value;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");
const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// The preset's declared changes from Appendix A, as TOML values:
/// `(key, Appendix A's value, the preset's value)`.
const DECLARED: [(&str, &str, &str); 8] = [
    (
        "name",
        "\"reference-field-ecology\"",
        "\"reference-small-world\"",
    ),
    ("chemistry.uptake_capacity_per_second", "0.4", "6.0"),
    ("sources.power_per_source", "80.0", "960.0"),
    ("costs.basal_energy_per_second", "0.005", "0.8"),
    (
        "reproduction.minimum_parent_energy_before_attempt",
        "4.0",
        "3.0",
    ),
    ("reproduction.cooldown_seconds", "1.0", "0.5"),
    ("chemistry.motion_response_norm_limit", "1.0", "256.0"),
    (
        "execution.command_durability",
        "\"write-ahead-before-apply\"",
        "\"volatile-in-memory\"",
    ),
];

/// The smoke run's committed world after 1,024 ticks, identical on every
/// qualified host.
const SMOKE_DIGEST: &str = "2ade32c2f3e74f6f54181019774181e2aa0857bd79bd6c5a6f774f85e46a8183";

fn appendix_a() -> &'static str {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    &appendix[start..start + length]
}

fn preset_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../presets/reference-small-world.toml")
}

/// Every leaf of a TOML document by its dotted key.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn leaves(text: &str) -> BTreeMap<String, toml::Value> {
    fn walk(prefix: &str, table: &toml::Table, out: &mut BTreeMap<String, toml::Value>) {
        for (key, value) in table {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            match value {
                toml::Value::Table(inner) => walk(&path, inner, out),
                leaf => {
                    out.insert(path, leaf.clone());
                }
            }
        }
    }
    let table: toml::Table = toml::from_str(text).expect("TOML");
    let mut out = BTreeMap::new();
    walk("", &table, &mut out);
    out
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn value(text: &str) -> toml::Value {
    let table: toml::Table = toml::from_str(&format!("v = {text}")).expect("a TOML value");
    table.get("v").cloned().expect("v")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn preset() -> ValidatedConfig {
    ValidatedConfig::from_toml(PRESET).expect("the preset validates")
}

/// The preset is Appendix A with exactly its declared changes, and its
/// header declares each one.
#[test]
fn the_preset_is_appendix_a_with_exactly_its_declared_changes() {
    let appendix = leaves(appendix_a());
    let preset = leaves(PRESET);
    assert_eq!(
        appendix.keys().collect::<Vec<_>>(),
        preset.keys().collect::<Vec<_>>()
    );
    let changed: Vec<(&str, &toml::Value, &toml::Value)> = appendix
        .iter()
        .filter(|(key, value)| preset[*key] != **value)
        .map(|(key, value)| (key.as_str(), value, &preset[key]))
        .collect();
    let mut declared: Vec<(&str, toml::Value, toml::Value)> = DECLARED
        .iter()
        .map(|&(key, from, to)| (key, value(from), value(to)))
        .collect();
    declared.sort_by(|a, b| a.0.cmp(b.0));
    assert_eq!(changed.len(), declared.len(), "{changed:?}");
    for ((key, from, to), (declared_key, declared_from, declared_to)) in
        changed.iter().zip(&declared)
    {
        assert_eq!(key, declared_key);
        assert_eq!(*from, declared_from, "{key}");
        assert_eq!(*to, declared_to, "{key}");
    }
    let header: Vec<&str> = PRESET
        .lines()
        .take_while(|line| line.starts_with('#'))
        .collect();
    for (key, from, to) in DECLARED {
        let (from, to) = (from.trim_matches('"'), to.trim_matches('"'));
        assert!(
            header.iter().any(|line| line.contains(key)
                && line.contains(from)
                && line.contains(to)
                && line.contains('→')),
            "the header does not declare {key}: {from} → {to}"
        );
    }
}

/// The preset validates at the Reference profile's scale (§19.5), and its
/// session admits it. Appendix A as written is refused only for its
/// durability, which needs FL-036's journal.
#[test]
fn the_preset_validates_at_the_reference_scale_and_its_session_admits_it() {
    let config = preset();
    let summary = Validation::of(&config);
    assert_eq!(summary.schema, VALIDATE_SCHEMA);
    assert_eq!(summary.name, "reference-small-world");
    assert_eq!(summary.model, "field-ecology-v1");
    assert_eq!(summary.seed, 42);
    assert_eq!(summary.grid, [128, 128]);
    assert_eq!(summary.founders, 4096);
    assert_eq!(summary.maximum_population, 16_384);
    assert_eq!(summary.timestep, [1, 128]);
    assert_eq!(summary.default_parameters, 9988);
    assert_eq!(summary.initial_genome_bytes, 4096 * 9988 * 4);
    assert_eq!(summary.worst_genome_bytes, 16_384 * 30_000 * 4);
    assert!(!summary.worst_case_fits_logical_quota);
    assert_eq!(summary.cooldown_ticks, 64);
    assert_eq!(summary.command_durability, "volatile-in-memory");
    assert!(SessionConfig::from_config(RUN_SESSION, &config).is_ok());

    let appendix = ValidatedConfig::from_toml(appendix_a());
    assert!(appendix.is_ok(), "{appendix:?}");
    let refused = appendix.map(|appendix| SessionConfig::from_config(RUN_SESSION, &appendix));
    assert!(
        matches!(refused, Ok(Err(CoreError::MissingCapability { .. }))),
        "{refused:?}"
    );
}

/// Run the preset for `ticks`, collecting every record.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn smoke(ticks: u64, every: u64) -> (Finished, Vec<Record>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    let finished = run(
        &preset(),
        RunOptions {
            ticks,
            every,
            cpu_workers: None,
            controllers: ControllerChoice::Cpu,
        },
        move |record| {
            sink.lock().expect("records").push(record.clone());
        },
    )
    .expect("the smoke run runs");
    let records = records.lock().expect("records").clone();
    (finished, records)
}

#[expect(clippy::cast_precision_loss, reason = "counts here are far below 2^53")]
fn fraction(part: u64, whole: u64) -> f64 {
    part as f64 / whole as f64
}

/// G2's first world in raw state: births, deaths, paid emission and
/// heritable variation within 1,024 ticks of the real preset, through the
/// session owner.
#[test]
fn the_smoke_run_shows_births_deaths_emissions_and_heritable_variation_in_raw_state() {
    let (finished, records) = smoke(1024, 256);
    let Finished {
        stop,
        tally,
        survey,
        closed,
    } = &finished;
    assert_eq!(*stop, Stop::Reached);
    assert_eq!(closed.boundary.tick.get(), 1024);
    assert_eq!(tally.ticks, 1024);
    assert!(closed.book.is_closed());

    // The records: one start, a progress record every 256 ticks, one end
    // carrying the same tally and survey.
    assert!(
        matches!(&records[0], Record::Start { schema, ticks: 1024, founders: 4096, .. } if *schema == RUN_SCHEMA)
    );
    let progress: Vec<u64> = records
        .iter()
        .filter_map(|record| match record {
            Record::Progress { tick, .. } => Some(*tick),
            _ => None,
        })
        .collect();
    assert_eq!(progress, [256, 512, 768, 1024]);
    match records.last() {
        Some(Record::End {
            stop: end,
            tick,
            digest,
            tally: end_tally,
            survey: end_survey,
            ..
        }) => {
            assert_eq!(end, stop);
            assert_eq!(*tick, 1024);
            assert_eq!(*digest, closed.world.digest().to_hex());
            assert_eq!(**end_tally, *tally);
            assert_eq!(**end_survey, *survey);
        }
        other => panic!("the last record is not the end: {other:?}"),
    }
    assert_eq!(records.len(), 6);

    // Births, deaths and emissions occurred, and the committed world's raw
    // state agrees with the reports: every identity issued is a founder or
    // a reported birth, and every one not alive is a reported death.
    assert!(tally.births > 0, "{tally:?}");
    assert!(tally.deaths > 0, "{tally:?}");
    assert!(
        tally.emitter_ticks > 0 && tally.emission_paid > 0.0,
        "{tally:?}"
    );
    assert!(tally.emission_deposited > 0.0);
    assert!(tally.emission_deposited <= 0.9 * tally.emission_paid * (1.0 + 1.0e-12));
    assert!(tally.child_endowment > 0.0);
    // Every death here is exhaustion, at the stage named, which leaves no
    // residual and so recycles nothing (FL-012). Only an explicit removal
    // recycles, and removal is not a command yet (FL-022).
    assert!(
        tally.deaths_by_stage.keys().all(|stage| [
            "chemistry",
            "maintenance",
            "motion",
            "emission",
            "split"
        ]
        .contains(stage)),
        "{tally:?}"
    );
    assert_eq!(tally.recycled.to_bits(), 0.0_f64.to_bits());
    assert_eq!(tally.rejected_births, 0);
    assert_eq!(survey.identities_issued, 4096 + tally.births);
    assert_eq!(
        u64::try_from(survey.live).ok(),
        Some(4096 + tally.births - tally.deaths)
    );
    assert_eq!(closed.world.population().len(), survey.live);
    assert_eq!(*survey, Survey::of(&closed.world));
    assert_eq!(
        survey.founders_alive + survey.descendants_alive,
        survey.live
    );
    assert!(
        survey.descendants_alive > 0 && survey.max_generation >= 3,
        "{survey:?}"
    );
    assert_eq!(survey.distinct_genomes, survey.live);
    assert_eq!(survey.distinct_parameter_sets, survey.live);
    assert_eq!(survey.distinct_trait_vectors, survey.live);
    let normalized = tally.normalized_residual.unwrap_or(f64::INFINITY);
    assert!(normalized.abs() < 1.0e-5, "{tally:?}");

    // Heritable variation, from the raw genomes alone. Every live child
    // differs from its live parent and equals it in all but a few
    // parameters; unrelated founders share almost none. In trait space a
    // child sits far closer to its parent than founders sit to each other.
    let heredity = &survey.heredity;
    assert!(heredity.pairs >= 100, "{heredity:?}");
    assert_eq!(heredity.pairs_differing, heredity.pairs);
    let kept = fraction(heredity.pair_parameters_identical, heredity.pair_parameters);
    let shared = fraction(
        heredity.founder_parameters_identical,
        heredity.founder_parameters,
    );
    assert!(kept > 0.99, "{heredity:?}");
    assert!(shared < 0.05, "{heredity:?}");
    assert!(heredity.most_parameter_differences < 200, "{heredity:?}");
    let pair = heredity.pair_trait_distance.unwrap_or(f64::INFINITY);
    let founders = heredity.founder_trait_distance.unwrap_or(0.0);
    assert!(pair < 0.1 * founders, "{heredity:?}");

    // The variation is inherited: a live grandchild carries almost every
    // parameter in which its parent differs from its grandparent, and sits
    // closer to its parent than to its grandparent in trait space.
    assert!(heredity.triples >= 20, "{heredity:?}");
    let carried = fraction(
        heredity.carried_parameter_variants,
        heredity.parental_parameter_variants,
    );
    assert!(carried > 0.99, "{heredity:?}");
    let to_parent = heredity
        .triple_trait_distance_to_parent
        .unwrap_or(f64::INFINITY);
    let to_grandparent = heredity.triple_trait_distance_to_grandparent.unwrap_or(0.0);
    assert!(to_parent < to_grandparent, "{heredity:?}");

    // The run is reproducible across hosts. The digest pins the committed
    // world; the tally and survey pin what the observer read from the reports
    // and from that world, every float included (the survey's logarithm is
    // the host-independent one).
    assert_eq!(closed.world.digest().to_hex(), SMOKE_DIGEST);
    assert_eq!(
        *tally,
        Tally {
            ticks: 1024,
            births: 507,
            rejected_births: 0,
            deaths: 3775,
            deaths_by_stage: BTreeMap::from([
                ("chemistry", 159),
                ("maintenance", 3613),
                ("motion", 3)
            ]),
            emitter_ticks: 747_053,
            emission_paid: 1_008.661_307_687_298_5,
            emission_deposited: 907.713_231_889_463_1,
            child_endowment: 1_305.556_324_834_606_6,
            recycled: 0.0,
            perturbed_parameters: 10_191,
            replaced_parameters: 530,
            topology_changes: 6,
            rejected_candidates: 0,
            peak_live: 4096,
            least_live: Some(478),
            normalized_residual: Some(7.267_934_839_627_371e-10),
        }
    );
    assert_eq!(
        *survey,
        Survey {
            tick: 1024,
            live: 828,
            founders_alive: 323,
            descendants_alive: 505,
            max_generation: 5,
            generations: BTreeMap::from([(0, 323), (1, 277), (2, 160), (3, 53), (4, 12), (5, 3)]),
            lineages_alive: 325,
            identities_issued: 4603,
            distinct_genomes: 828,
            distinct_parameter_sets: 828,
            distinct_trait_vectors: 828,
            topologies: BTreeMap::from([(0, 4), (1, 3), (2, 818), (4, 3)]),
            heredity: Heredity {
                pairs: 497,
                pairs_differing: 497,
                pairs_across_topologies: 6,
                pair_parameters: 4_985_044,
                pair_parameters_identical: 4_974_500,
                most_parameter_differences: 69,
                pair_trait_distance: Some(0.058_025_554_391_786_26),
                founder_pairs: 322,
                founder_parameters: 3_216_136,
                founder_parameters_identical: 42_504,
                founder_trait_distance: Some(1.985_464_526_404_333_4),
                triples: 219,
                parental_parameter_variants: 4596,
                carried_parameter_variants: 4583,
                triple_trait_distance_to_parent: Some(0.058_060_253_606_112_72),
                triple_trait_distance_to_grandparent: Some(0.081_480_224_578_190_21),
            },
        }
    );
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
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

/// The `flife` binary validates and runs the preset as versioned JSON, and
/// refuses an unreadable file, an invalid configuration and an unadmitted
/// durability with typed error records.
#[test]
fn the_flife_binary_validates_runs_and_refuses_with_typed_errors() {
    let path = preset_path();
    let path = path.to_str().expect("a UTF-8 path");

    let (status, out, err) = flife(&["validate", path]);
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["schema"], VALIDATE_SCHEMA);
    assert_eq!(out[0]["name"], "reference-small-world");
    assert_eq!(
        out[0]["config_hash"],
        preset().config_hash().to_hex().as_str()
    );

    let (status, out, err) = flife(&["run", path, "--ticks", "2", "--every", "1"]);
    assert_eq!(status, Some(0), "{err:?}");
    let kinds: Vec<&str> = out
        .iter()
        .map(|record| record["record"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(kinds, ["start", "progress", "progress", "end"]);
    assert!(out.iter().all(|record| record["schema"] == RUN_SCHEMA));
    assert_eq!(out[3]["stop"]["end"], "reached");
    assert_eq!(out[3]["tick"], 2);
    assert_eq!(out[3]["survey"]["live"], out[2]["live"]);

    let (status, out, err) = flife(&["validate", "/nonexistent/preset.toml"]);
    assert_eq!(status, Some(2));
    assert!(out.is_empty());
    assert_eq!(err[0]["schema"], ERROR_SCHEMA);
    assert_eq!(err[0]["kind"], "io");

    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let invalid = scratch.join("fl015-unknown-key.toml");
    std::fs::write(
        &invalid,
        PRESET.replace("[world]\n", "[world]\nbogus = 1\n"),
    )
    .expect("write the invalid configuration");
    let (status, _, err) = flife(&["validate", invalid.to_str().expect("UTF-8")]);
    assert_eq!(status, Some(2));
    assert_eq!(err[0]["kind"], "invalid-config");

    let appendix = scratch.join("fl015-appendix-a.toml");
    std::fs::write(&appendix, appendix_a()).expect("write Appendix A");
    let appendix = appendix.to_str().expect("UTF-8");
    let (status, _, err) = flife(&["validate", appendix]);
    assert_eq!(status, Some(0), "{err:?}");
    let (status, out, err) = flife(&["run", appendix, "--ticks", "1"]);
    assert_eq!(status, Some(2));
    assert!(out.is_empty());
    assert_eq!(err[0]["kind"], "missing-capability");
}

/// A run that pauses on capacity reports it and exits with status 1: with
/// the ceiling at the founders' count, the first viable birth has no slot
/// (`pause-before-commit`), and nothing after the last published tick
/// changes.
///
/// The construction needs the first viable birth before the first death. At
/// Appendix A's motility gain (1.0) the first birth comes by tick 144 and
/// the first death by tick 168. At the preset's gain a poisoning death comes
/// first (by tick 160, the first birth by 176) and frees a slot, so this
/// test keeps Appendix A's gain: its subject is the pause, not the ecology.
#[test]
fn a_capacity_pause_ends_the_run_with_its_stop_and_status_one() {
    let full = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fl015-no-room.toml");
    let configuration = PRESET
        .replace("maximum_count = 16384\n", "maximum_count = 4096\n")
        .replace(
            "motion_response_norm_limit = 256.0\n",
            "motion_response_norm_limit = 1.0\n",
        );
    assert!(configuration.contains("\nmaximum_count = 4096\n"));
    assert!(configuration.contains("\nmotion_response_norm_limit = 1.0\n"));
    std::fs::write(&full, configuration).expect("write the configuration");
    let (status, out, err) = flife(&[
        "run",
        full.to_str().expect("UTF-8"),
        "--ticks",
        "1024",
        "--every",
        "0",
    ]);
    assert_eq!(status, Some(1), "{err:?}");
    assert_eq!(out.len(), 2);
    let end = &out[1];
    assert_eq!(end["record"], "end");
    assert_eq!(end["stop"]["end"], "capacity-paused");
    let tick = end["tick"].as_u64().expect("a tick");
    assert!(tick > 0 && tick < 1024, "{end}");
    assert_eq!(end["tally"]["ticks"], tick);
    assert_eq!(end["tally"]["births"], 0);
    assert_eq!(
        end["survey"]["live"],
        4096 - end["tally"]["deaths"].as_u64().expect("deaths")
    );
}
