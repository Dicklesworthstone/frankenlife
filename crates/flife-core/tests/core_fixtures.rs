//! FL-002 fixtures: canonical serialization, integer clocks, the initial and
//! source algorithm registry, and checked bounds.
//!
//! The starter configuration is read from the plan's Appendix A at test
//! time, so the plan and the validator cannot drift apart silently.

use flife_core::canonical::canonical_float;
use flife_core::clock::{TickSpan, age_seconds, stable_phase};
use flife_core::ids::{Generation, IdSequence, OrganismId, SequentialId};
use flife_core::model::{dense_parameter_count, genome_bank_bytes};
use flife_core::registry::{self, REGISTRY, Role, Status};
use flife_core::schema::{self, Kind};
use flife_core::{
    ConfigIssue, CoreError, ModelDescriptor, RunConfig, Sha256Digest, Tick, TimeStep,
    ValidatedConfig, validate,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");
const THIS_FILE: &str = include_str!("core_fixtures.rs");

/// The test files a registry `test:<crate>::<path>::<fn>` citation may name.
const EVIDENCE_FILES: &[(&str, &str)] = &[
    ("flife-core::tests/core_fixtures.rs", THIS_FILE),
    (
        "flife-field::tests/field_layout.rs",
        include_str!("../../flife-field/tests/field_layout.rs"),
    ),
    (
        "flife-field::tests/transport_sources.rs",
        include_str!("../../flife-field/tests/transport_sources.rs"),
    ),
    (
        "flife-field::tests/uptake_ledger.rs",
        include_str!("../../flife-field/tests/uptake_ledger.rs"),
    ),
    ("flife-core::tests/numerics.rs", include_str!("numerics.rs")),
    (
        "flife-engine::tests/organism_phase.rs",
        include_str!("../../flife-engine/tests/organism_phase.rs"),
    ),
    (
        "flife-engine::tests/world.rs",
        include_str!("../../flife-engine/tests/world.rs"),
    ),
    (
        "flife-runtime::tests/book.rs",
        include_str!("../../flife-runtime/tests/book.rs"),
    ),
    (
        "flife-runtime::tests/session.rs",
        include_str!("../../flife-runtime/tests/session.rs"),
    ),
    (
        "flife-runtime::tests/observation.rs",
        include_str!("../../flife-runtime/tests/observation.rs"),
    ),
    (
        "flife-neural::tests/sensing.rs",
        include_str!("../../flife-neural/tests/sensing.rs"),
    ),
    (
        "flife-neural::tests/controller.rs",
        include_str!("../../flife-neural/tests/controller.rs"),
    ),
    (
        "flife-genome::tests/genomes.rs",
        include_str!("../../flife-genome/tests/genomes.rs"),
    ),
    ("flife-core::tests/rng.rs", include_str!("rng.rs")),
    (
        "flife-genome::tests/mutation.rs",
        include_str!("../../flife-genome/tests/mutation.rs"),
    ),
    (
        "flife-field::tests/seeding.rs",
        include_str!("../../flife-field/tests/seeding.rs"),
    ),
    (
        "flife-engine::tests/emission_births.rs",
        include_str!("../../flife-engine/tests/emission_births.rs"),
    ),
    (
        "flife-engine::tests/lifecycle.rs",
        include_str!("../../flife-engine/tests/lifecycle.rs"),
    ),
];

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn appendix_a_toml() -> &'static str {
    let appendix = &PLAN[PLAN.find("## Appendix A").expect("Appendix A heading")..];
    let start = appendix.find("```toml\n").expect("Appendix A TOML block") + "```toml\n".len();
    let length = appendix[start..].find("```").expect("closing fence");
    &appendix[start..start + length]
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn starter() -> RunConfig {
    RunConfig::from_toml(appendix_a_toml()).expect("Appendix A parses")
}

#[expect(
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn issues_after(edit: impl FnOnce(&mut RunConfig)) -> Vec<ConfigIssue> {
    let mut config = starter();
    edit(&mut config);
    match validate(config) {
        Err(CoreError::InvalidConfig(issues)) => issues,
        Err(other) => panic!("expected InvalidConfig, got {other}"),
        Ok(_) => panic!("the edited configuration was accepted"),
    }
}

fn assert_refused(edit: impl FnOnce(&mut RunConfig), path: &str, needle: &str) {
    let issues = issues_after(edit);
    assert!(
        issues
            .iter()
            .any(|issue| issue.path == path && issue.problem.contains(needle)),
        "expected an issue at {path} containing {needle:?}; got {issues:?}"
    );
}

// ------------------------------------------------------------ starter

#[test]
fn appendix_a_starter_validates() {
    let validated = ValidatedConfig::from_toml(appendix_a_toml()).expect("Appendix A is valid");
    let report = validated.report();
    assert_eq!(
        (report.timestep.numerator(), report.timestep.denominator()),
        (1, 128)
    );
    assert_eq!((report.cell_width, report.cell_height), (1.0, 1.0));
    // Sensing scales 1..16 world units over unit cells: levels 2^0..2^4.
    assert_eq!(report.pyramid_levels, 4);
    assert_eq!(report.cooldown_ticks, TickSpan::new(128));
    assert_eq!(report.source_segment_ticks, TickSpan::new(3840));
    assert_eq!(report.checkpoint_interval_ticks, TickSpan::new(3840));
    // §6.2: 104(64)+64+64(32)+32+32(36)+36.
    assert_eq!(report.default_parameter_count, 9_988);
    assert_eq!(report.initial_genome_bytes, 4_096 * 9_988 * 4);
    assert_eq!(report.default_genome_bytes_at_maximum, 654_573_568);
    assert_eq!(
        report.parameter_cap_genome_bytes_at_maximum,
        16_384 * 30_000 * 4
    );
    // A.2: the population ceiling does not promise every organism can carry
    // the largest topology; the 1 GiB logical quota binds first.
    assert!(!report.worst_case_fits_logical_quota);
}

#[test]
fn catalog_parameter_counts_match_section_6_4() {
    let catalog: [(&[u32], u64); 5] = [
        (&[8], 1_164),
        (&[32], 4_548),
        (&[64, 32], 9_988),
        (&[96, 64], 18_628),
        (&[128, 96], 29_316),
    ];
    for (hidden, expected) in catalog {
        assert_eq!(
            dense_parameter_count(104, hidden, 36).unwrap(),
            expected,
            "{hidden:?}"
        );
    }
    assert!(dense_parameter_count(104, &[0], 36).is_err());
    assert!(matches!(
        dense_parameter_count(u32::MAX, &[u32::MAX, u32::MAX], u32::MAX),
        Err(CoreError::Overflow { .. })
    ));
}

#[test]
fn a2_largest_topology_overcommits_the_logical_quota() {
    // A.2: 16,384 organisms at 29,316 F32 parameters need 1,832.25 MiB.
    let bytes = genome_bank_bytes(16_384, 29_316, 4).unwrap();
    assert_eq!(bytes, 1_921_253_376);
    assert_eq!(bytes * 4, 1_832 * 4 * 1_048_576 + 1_048_576);
    assert!(bytes > 1_073_741_824);
    assert!(genome_bank_bytes(u64::MAX, 2, 4).is_err());
}

// ------------------------------------------------------- canonical form

#[test]
fn sha256_matches_the_nist_vector() {
    assert_eq!(
        Sha256Digest::of(b"abc").to_hex(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn canonical_floats_round_trip_and_fold_negative_zero() {
    assert_eq!(canonical_float(-0.0), "0.0");
    assert_eq!(canonical_float(128.0), "128.0");
    assert_eq!(canonical_float(0.15), "0.15");
    assert_eq!(canonical_float(1.0e-12), "1e-12");
    for value in [0.15, 1.0e-12, 2.0e-9, 1.0 / 3.0, 1.0e300, f64::MIN_POSITIVE] {
        let text = canonical_float(value);
        let parsed: f64 = text.parse().unwrap();
        assert_eq!(parsed.to_bits(), value.to_bits(), "{text}");
    }
}

#[test]
fn canonical_toml_round_trips_and_is_idempotent() {
    let first = ValidatedConfig::from_toml(appendix_a_toml()).unwrap();
    let text = first.canonical_toml();
    let second = ValidatedConfig::from_toml(&text).expect("canonical TOML is valid");
    assert_eq!(second.config(), first.config());
    assert_eq!(second.canonical_toml(), text);
    assert_eq!(second.config_hash(), first.config_hash());
    assert!(text.starts_with("schema = \"frankenlife.config.v1\"\n"));
    assert!(text.contains("\n[world]\nwidth = 128.0\n"));
    assert!(text.contains("tiny_energy_threshold = 1e-12\n"));
}

#[test]
fn canonical_form_and_hashes_are_pinned() {
    // Regression pins: they change only when the canonical form, schema
    // table or Appendix A changes, and such a change must be deliberate.
    // Both values were checked with `shasum -a 256` over the written bytes.
    let config = ValidatedConfig::from_toml(appendix_a_toml()).unwrap();
    let descriptor = ModelDescriptor::of(&config);
    assert_eq!(config.config_hash().to_string(), PINNED_CONFIG_HASH);
    assert_eq!(descriptor.digest().to_string(), PINNED_MODEL_DIGEST);
}

const PINNED_CONFIG_HASH: &str =
    "sha256:67a0986b27b21780730e22a0e59d673baa578f41641b0a07d99a578b7b249cc3";
const PINNED_MODEL_DIGEST: &str =
    "sha256:28929a50249d0af51de18407bf92d516a834769571d23138afa0de9dd70cd62e";

#[test]
fn unknown_and_missing_keys_are_refused() {
    let unknown = appendix_a_toml().replace("[world]\n", "[world]\nbogus = 1\n");
    let error = RunConfig::from_toml(&unknown).unwrap_err();
    assert!(
        error.to_string().contains("unknown field `bogus`"),
        "{error}"
    );

    let missing = appendix_a_toml().replace("diffusion = 0.15\n", "");
    let error = RunConfig::from_toml(&missing).unwrap_err();
    assert!(
        error.to_string().contains("missing field `diffusion`"),
        "{error}"
    );
}

// ------------------------------------------------- A.2 checked bounds

#[test]
fn nonfinite_and_out_of_bound_numbers_are_refused() {
    assert_refused(
        |c| c.world.diffusion = f64::NAN,
        "world.diffusion",
        "finite",
    );
    assert_refused(
        |c| c.costs.birth_energy = -0.1,
        "costs.birth_energy",
        ">= 0",
    );
    assert_refused(
        |c| c.chemistry.maximum_efficiency = 1.5,
        "chemistry.maximum_efficiency",
        "(0, 1]",
    );
    assert_refused(
        |c| c.chemistry.discrimination_threshold = 0.0,
        "chemistry.discrimination_threshold",
        "> 0",
    );
    assert_refused(
        |c| c.chemistry.toxicity_sensitivity = 0.0,
        "chemistry.toxicity_sensitivity",
        "> 0",
    );
    assert_refused(
        |c| c.chemistry.uptake_half_saturation_density = 0.0,
        "chemistry.uptake_half_saturation_density",
        "> 0",
    );
    assert_refused(
        |c| c.chemistry.maximum_speed = 0.0,
        "chemistry.maximum_speed",
        "> 0",
    );
    assert_refused(|c| c.seed = u64::MAX, "seed", "TOML integer range");
    // One displayable render epoch and one being written (§9.5, FL-017).
    assert_refused(
        |c| c.execution.render_queue_capacity = 1,
        "execution.render_queue_capacity",
        "at least 2, got 1",
    );
}

#[test]
fn model_abi_and_catalog_violations_are_refused() {
    assert_refused(
        |c| c.world.moment_channels = 16,
        "world.moment_channels",
        "32 channels",
    );
    assert_refused(
        |c| c.controller.input_width = 100,
        "controller.input_width",
        "104 inputs",
    );
    assert_refused(
        |c| c.controller.output_width = 35,
        "controller.output_width",
        "36 outputs",
    );
    assert_refused(
        |c| c.controller.hidden_widths = vec![64, 32, 16],
        "controller.hidden_widths",
        "1 or 2 hidden layers",
    );
    assert_refused(
        |c| c.architecture_mutation.maximum_parameters = 5_000,
        "architecture_mutation.maximum_parameters",
        "9988 parameters",
    );
    assert_refused(
        |c| c.controller.autograd = true,
        "controller.autograd",
        "gradient",
    );
    assert_refused(
        |c| c.mutation.meta_evolution = true,
        "mutation.meta_evolution",
        "not part",
    );
    assert_refused(
        |c| c.population.automatic_reseed = true,
        "population.automatic_reseed",
        "never reseeds",
    );
    assert_refused(
        |c| c.population.initial_count = 20_000,
        "population.initial_count",
        "exceeds maximum_count",
    );
}

#[test]
fn unknown_and_misplaced_algorithms_are_refused() {
    assert_refused(
        |c| c.world.exchange_kernel = "grid-cic-v2".into(),
        "world.exchange_kernel",
        "not a registered",
    );
    assert_refused(
        |c| c.world.transport_scheme = "grid-cic-v1".into(),
        "world.transport_scheme",
        "registered only for world.exchange_kernel",
    );
    assert_refused(
        |c| c.controller.canonical_weight_dtype = "f64".into(),
        "controller.canonical_weight_dtype",
        "not a registered",
    );
    assert_refused(
        |c| c.chemistry.heritable_traits.push("motion_response".into()),
        "chemistry.heritable_traits",
        "twice",
    );
    assert_refused(
        |c| c.schema = "frankenlife.config.v2".into(),
        "schema",
        "frankenlife.config.v1",
    );
    assert_refused(|c| c.name = "bad name".into(), "name", "[A-Za-z0-9._-]");
    assert_refused(
        |c| c.presentation.background = "#GGGGGG".into(),
        "presentation.background",
        "#rrggbb",
    );
}

#[test]
fn birth_viability_and_strict_mutation_are_checked() {
    // E − c_b = 3.95: a viable child needs β ≥ 0.506 and a viable parent β ≤ 0.494.
    assert_refused(
        |c| {
            c.reproduction.minimum_child_energy = 2.0;
            c.reproduction.minimum_parent_energy_after_split = 2.0;
        },
        "reproduction.minimum_parent_energy_before_attempt",
        "viable",
    );
    assert_refused(
        |c| {
            c.reproduction.minimum_child_fraction = 0.9;
            c.reproduction.maximum_child_fraction = 0.1;
        },
        "reproduction.minimum_child_fraction",
        "minimum <= maximum",
    );
    let silence = |c: &mut RunConfig| {
        c.mutation.weight_probability = 0.0;
        c.mutation.replacement_probability = 0.0;
        c.mutation.chemistry_standard_deviation = 0.0;
        c.mutation.sensing_log_scale_standard_deviation = 0.0;
        c.architecture_mutation.probability_per_birth = 0.0;
    };
    assert_refused(
        silence,
        "mutation.reject_effective_noop",
        "every birth fails",
    );
    // Perturbation with a zero deviation changes nothing either.
    assert_refused(
        |c: &mut RunConfig| {
            silence(c);
            c.mutation.weight_probability = 1.0;
            c.mutation.weight_standard_deviation = 0.0;
        },
        "mutation.reject_effective_noop",
        "every birth fails",
    );
    // Positive control: without the strict no-op rule, clonal births are legal.
    let mut config = starter();
    silence(&mut config);
    config.mutation.reject_effective_noop = false;
    assert!(validate(config).is_ok());
    // Strict mutation tries at most eight candidates per attempted birth (§7.3).
    assert_refused(
        |c: &mut RunConfig| c.mutation.maximum_candidate_proposals_per_birth = 9,
        "mutation.maximum_candidate_proposals_per_birth",
        "at most eight",
    );
    let mut config = starter();
    config.mutation.maximum_candidate_proposals_per_birth = 8;
    assert!(validate(config).is_ok());
}

#[test]
fn geometry_pyramid_source_and_tick_alignment_are_checked() {
    assert_refused(
        |c| {
            c.world.grid_width = 120;
            c.world.width = 120.0;
        },
        "world.grid_width",
        "divisible by 16",
    );
    assert_refused(
        |c| c.sensors.minimum_scale_world_units = 0.5,
        "sensors.minimum_scale_world_units",
        "finest pyramid level",
    );
    assert_refused(
        |c| c.sensors.initial_scale_world_units = 20.0,
        "sensors.initial_scale_world_units",
        "minimum <= initial <= maximum",
    );
    assert_refused(
        |c| c.sources.kernel_cutoff_sigma = 0.05,
        "sources.kernel_cutoff_sigma",
        "miss every cell center",
    );
    assert_refused(
        |c| c.sources.path_direction_interval_seconds = 30.001,
        "sources.path_direction_interval_seconds",
        "whole number",
    );
    // 0.1 s is slightly more than one 1/10 s tick in binary; refuse rather
    // than silently charge a second tick of cooldown.
    assert_refused(
        |c| {
            c.world.tick_denominator = 10;
            c.sources.path_direction_interval_seconds = 30.0;
            c.reproduction.cooldown_seconds = 0.1;
        },
        "reproduction.cooldown_seconds",
        "within f64 rounding of 1 ticks",
    );
}

#[test]
fn memory_quotas_are_simultaneous_constraints() {
    assert_refused(
        |c| c.execution.maximum_logical_genome_bytes = 100_000_000,
        "execution.maximum_logical_genome_bytes",
        "initial population",
    );
    assert_refused(
        |c| c.execution.maximum_allocated_genome_bytes = 4_000_000_000,
        "execution.maximum_allocated_genome_bytes",
        "maximum_total_bytes",
    );
    assert_refused(
        |c| c.execution.checkpoint_retained_bytes = 3_000_000_000,
        "execution.checkpoint_retained_bytes",
        "maximum_total_bytes",
    );
}

#[test]
fn every_issue_is_reported_not_just_the_first() {
    let issues = issues_after(|c| {
        c.world.diffusion = -1.0;
        c.costs.basal_energy_per_second = -1.0;
        c.controller.autograd = true;
    });
    let paths: Vec<&str> = issues.iter().map(|issue| issue.path.as_str()).collect();
    for expected in [
        "world.diffusion",
        "costs.basal_energy_per_second",
        "controller.autograd",
    ] {
        assert!(
            paths.contains(&expected),
            "{expected} missing from {paths:?}"
        );
    }
}

// ----------------------------------------------------- integer clocks

#[test]
fn integer_clocks_keep_advancing_where_f32_time_stalls() {
    // C.15: fast-forward 16,777,216 ticks at Δt = 1/128.
    let step = TimeStep::new(1, 128).unwrap();
    let now = Tick::new(1 << 24);
    assert_eq!(age_seconds(Tick::ZERO, now, step).unwrap(), 131_072.0);
    assert_eq!(
        age_seconds(Tick::ZERO, now.next().unwrap(), step).unwrap(),
        131_072.0078125
    );

    // The independent oracle for the plan's warning: an F32 accumulator.
    let mut accumulated = 0.0_f32;
    for _ in 0..(1_u32 << 24) {
        accumulated += 1.0 / 128.0;
    }
    assert_eq!(accumulated, 131_072.0);
    assert_eq!(
        accumulated + 1.0 / 128.0,
        accumulated,
        "F32 time has stalled"
    );
}

#[test]
fn tick_overflow_is_refused_without_simulating_history() {
    let last = Tick::new(u64::MAX);
    assert!(matches!(last.next(), Err(CoreError::Overflow { .. })));
    assert!(
        Tick::new(u64::MAX - 5)
            .checked_add(TickSpan::new(10))
            .is_err()
    );
    assert!(Tick::new(5).since(Tick::new(6)).is_err());
    let step = TimeStep::new(1, 128).unwrap();
    assert!(matches!(
        step.ticks_ceil(1.0e300, "x"),
        Err(CoreError::Overflow { .. })
    ));
}

#[test]
fn seconds_convert_to_ticks_exactly() {
    let step = TimeStep::new(2, 256).unwrap();
    assert_eq!((step.numerator(), step.denominator()), (1, 128));
    assert_eq!(
        step.ticks_ceil(1.0, "cooldown").unwrap(),
        TickSpan::new(128)
    );
    assert_eq!(
        step.ticks_exact(30.0, "segment").unwrap(),
        TickSpan::new(3840)
    );
    assert!(step.ticks_exact(30.001, "segment").is_err());
    // 30.001 s = 3840.128 ticks.
    assert_eq!(step.ticks_ceil(30.001, "x").unwrap(), TickSpan::new(3841));
    assert_eq!(step.ticks_ceil(0.0, "x").unwrap(), TickSpan::new(0));
    assert_eq!(step.ticks_ceil(1.0e-300, "x").unwrap(), TickSpan::new(1));
    assert_eq!(
        step.ticks_ceil(f64::from_bits(1), "x").unwrap(),
        TickSpan::new(1)
    );
    for bad in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            step.ticks_ceil(bad, "x"),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }
    assert!(TimeStep::new(0, 128).is_err());
    assert!(TimeStep::new(1, 0).is_err());

    // Exactness is about the f64's binary value: 0.1 is slightly above 1/10.
    let tenth = TimeStep::new(1, 10).unwrap();
    assert!(tenth.ticks_exact(0.1, "x").is_err());
    assert_eq!(tenth.ticks_exact(0.5, "x").unwrap(), TickSpan::new(5));
    let third = TimeStep::new(1, 3).unwrap();
    assert_eq!(third.ticks_exact(30.0, "x").unwrap(), TickSpan::new(90));
}

#[test]
fn stable_phase_is_identity_mod_interval() {
    assert_eq!(stable_phase(OrganismId::new(10), 4).unwrap(), 2);
    assert_eq!(
        stable_phase(OrganismId::new(u64::MAX), 7).unwrap(),
        (u64::MAX % 7) as u32
    );
    for raw in [0, 1, 99, 12_345] {
        assert_eq!(stable_phase(OrganismId::new(raw), 1).unwrap(), 0);
    }
    assert!(stable_phase(OrganismId::new(3), 0).is_err());
    // Storage order is irrelevant: the phase is a function of identity alone.
    let ids = [5_u64, 17, 2, 40];
    let forward: Vec<u32> = ids
        .iter()
        .map(|&id| stable_phase(OrganismId::new(id), 3).unwrap())
        .collect();
    let reversed: Vec<u32> = ids
        .iter()
        .rev()
        .map(|&id| stable_phase(OrganismId::new(id), 3).unwrap())
        .collect();
    assert_eq!(forward.iter().rev().copied().collect::<Vec<_>>(), reversed);
}

// ---------------------------------------------------------- identities

#[test]
fn identities_are_never_reissued_and_overflow_is_refused() {
    let mut sequence = IdSequence::<OrganismId>::starting_at(u64::MAX - 1);
    assert_eq!(sequence.issue().unwrap().raw(), u64::MAX - 1);
    assert_eq!(sequence.issue().unwrap().raw(), u64::MAX);
    assert_eq!(sequence.next_raw(), None);
    assert!(matches!(
        sequence.issue(),
        Err(CoreError::Overflow {
            quantity: "organism id"
        })
    ));

    let mut resumed = IdSequence::<OrganismId>::resume(Some(7));
    assert_eq!(resumed.issue().unwrap(), OrganismId::new(7));
    assert_eq!(resumed.next_raw(), Some(8));

    assert_eq!(Generation::FOUNDER.child().unwrap(), Generation::new(1));
    assert!(Generation::new(u32::MAX).child().is_err());
    assert_eq!(OrganismId::new(42).to_string(), "organism:42");
}

// ------------------------------------------------------------ registry

#[test]
fn registry_names_are_unique_per_role_and_cite_their_owners() {
    for (index, entry) in REGISTRY.iter().enumerate() {
        assert!(
            REGISTRY[..index]
                .iter()
                .all(|other| (other.role, other.name) != (entry.role, entry.name)),
            "{} registered twice for {}",
            entry.name,
            entry.role.key()
        );
        assert!(!entry.contract.is_empty(), "{} has no contract", entry.name);
        let owner_ok = entry.owner.len() == 6
            && entry.owner.starts_with("FL-0")
            && entry.owner[3..].chars().all(|c| c.is_ascii_digit());
        assert!(owner_ok, "{} has owner {}", entry.name, entry.owner);
        if let Status::Implemented { evidence } = entry.status {
            let cited = evidence.strip_prefix("test:").unwrap();
            let (file, function) = cited.rsplit_once("::").unwrap();
            let source = EVIDENCE_FILES
                .iter()
                .find_map(|&(name, source)| (name == file).then_some(source));
            assert!(
                source.is_some_and(|source| source.contains(&format!("fn {function}()"))),
                "{} cites missing evidence {evidence}",
                entry.name
            );
        }
    }
    assert!(registry::lookup(Role::ExchangeKernel, "grid-cic-v1").is_ok());
    assert!(matches!(
        registry::lookup(Role::TransportScheme, "grid-cic-v1"),
        Err(CoreError::UnknownAlgorithm {
            role: "world.transport_scheme",
            ..
        })
    ));
}

#[test]
fn every_named_key_has_a_registered_role_at_its_path() {
    let config = starter();
    let mut named = 0;
    for field in schema::fields(&config) {
        if let Kind::Name(role) | Kind::NameList(role) = field.kind {
            assert_eq!(role.key(), field.path());
            named += 1;
        }
    }
    // Every role is used by exactly one key.
    let mut roles: Vec<Role> = REGISTRY.iter().map(|entry| entry.role).collect();
    roles.sort();
    roles.dedup();
    assert_eq!(named, roles.len());
}

#[test]
fn the_initial_and_source_algorithms_are_registered() {
    // A.3 items 1, 2, 7, 8, 9 and 10: the initial-condition and source
    // definitions that FL-002's registry must name.
    let starter = starter();
    for (role, name) in [
        (
            Role::InitialPositions,
            starter.population.initial_positions.as_str(),
        ),
        (
            Role::InitialVelocity,
            starter.population.initial_velocity.as_str(),
        ),
        (Role::Initializer, starter.controller.initializer.as_str()),
        (
            Role::SourceChemistry,
            starter.sources.chemical_directions.as_str(),
        ),
        (Role::SourcePath, starter.sources.path.as_str()),
        (Role::SourceKernel, starter.sources.kernel.as_str()),
        (Role::PhasePolicy, starter.controller.phase_policy.as_str()),
    ] {
        let entry = registry::lookup(role, name).unwrap();
        assert!(
            entry.contract.contains("A.3") || entry.contract.contains('§'),
            "{name}"
        );
    }
}

// --------------------------------------------------- model descriptor

#[test]
fn model_identity_excludes_values_and_presentation() {
    let base = ValidatedConfig::from_toml(appendix_a_toml()).unwrap();
    let base_model = ModelDescriptor::of(&base);

    let mut tweaked = starter();
    tweaked.world.diffusion = 0.2;
    tweaked.presentation.width = 3840;
    tweaked.controller.hidden_widths = vec![96, 64];
    let tweaked = validate(tweaked).unwrap();
    assert_ne!(tweaked.config_hash(), base.config_hash());
    assert_eq!(ModelDescriptor::of(&tweaked).digest(), base_model.digest());

    let text = base_model.canonical_text();
    assert!(text.starts_with("frankenlife.model-descriptor.v1\nmodel\tfield-ecology-v1\n"));
    assert!(text.contains("component\tworld.exchange_kernel\tgrid-cic-v1\n"));
    assert!(text.contains("parameter\tworld.diffusion\tfloat\tworld-length^2/simulation-second\n"));
    assert!(text.contains("parameter\tcosts.motion_energy_per_speed_squared_second\tfloat\t"));
    assert!(text.contains("component\texecution.determinism\tqualified-reference\n"));
    assert!(!text.contains("presentation."));
    assert!(!text.contains("execution.backend"));
    assert!(!text.contains("\tseed\t"));
    assert!(
        base_model
            .components()
            .iter()
            .any(|(path, entry)| path == "sources.path" && entry.name == "seeded-smooth-torus-v1")
    );
}

#[test]
fn errors_name_their_class() {
    let budget = CoreError::BudgetExhausted {
        budget: "logical genome bytes",
        required: 10,
        available: 4,
    };
    assert_eq!(
        budget.to_string(),
        "logical genome bytes budget exhausted: 10 required, 4 available"
    );
    assert_eq!(
        CoreError::Overflow { quantity: "tick" }.to_string(),
        "tick overflow refused"
    );

    // Machine output names each class by a stable kebab-case kind, distinct
    // for every variant (§14.6).
    let every = [
        (
            CoreError::MissingCapability {
                capability: "x".into(),
            },
            "missing-capability",
        ),
        (budget, "budget-exhausted"),
        (
            CoreError::SchemaMismatch {
                expected: "a".into(),
                found: "b".into(),
            },
            "schema-mismatch",
        ),
        (
            CoreError::IncompatibleCheckpoint { reason: "x".into() },
            "incompatible-checkpoint",
        ),
        (CoreError::QueueClosed { queue: "control" }, "queue-closed"),
        (
            CoreError::NumericalInvalid {
                quantity: "energy",
                detail: "NaN".into(),
            },
            "numerical-invalid",
        ),
        (CoreError::Cancelled { reason: "x".into() }, "cancelled"),
        (CoreError::Overflow { quantity: "tick" }, "overflow"),
        (
            CoreError::UnknownAlgorithm {
                role: "backend",
                name: "x".into(),
            },
            "unknown-algorithm",
        ),
        (CoreError::InvalidConfig(Vec::new()), "invalid-config"),
        (
            CoreError::LayoutMismatch {
                buffer: "field",
                detail: "x".into(),
            },
            "layout-mismatch",
        ),
        (
            CoreError::AllocationFailed {
                buffer: "field",
                bytes: 1,
            },
            "allocation-failed",
        ),
        (
            CoreError::StateConflict {
                operation: "grants",
                detail: "x".into(),
            },
            "state-conflict",
        ),
    ];
    let mut kinds = std::collections::BTreeSet::new();
    for (error, kind) in &every {
        assert_eq!(error.kind(), *kind, "{error:?}");
        assert!(
            kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
            "{kind}"
        );
        assert!(kinds.insert(*kind), "{kind} is not distinct");
    }
}
