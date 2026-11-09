//! The named-algorithm registry (plan Appendix A, A.3).
//!
//! Every algorithm, policy, schema and dtype that a configuration names
//! resolves here to one entry. Each entry gives the configuration key it may
//! appear under, the plan section that defines it, the work item that
//! implements it, and its status. An unknown name, or a known name used
//! under the wrong key, is an error, never permission to pick an arbitrary
//! algorithm. Changing a definition needs a new versioned name, because
//! renaming nothing would silently change model identity (A.3).

use crate::error::CoreError;

/// The configuration key a named algorithm may appear under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
#[expect(
    missing_docs,
    reason = "each variant is documented by its `key()` configuration path"
)]
pub enum Role {
    Model,
    Boundary,
    FieldDtype,
    TransportScheme,
    StabilityPolicy,
    ExchangeKernel,
    TinyEnergyPolicy,
    InitialVelocity,
    InitialPositions,
    CapacityPolicy,
    FieldComposition,
    SourceNormalization,
    SourceKernel,
    SourceChemistry,
    SourceChemistryPolicy,
    SourcePath,
    ChemistryFamily,
    HeritableTrait,
    ChemistryEvaluation,
    SensorSchema,
    SensorNormalization,
    PyramidFilter,
    PyramidBlend,
    ControllerFamily,
    HiddenActivation,
    OutputActivation,
    WeightDtype,
    AccumulationDtype,
    PhasePolicy,
    Initializer,
    TopologyCatalog,
    InheritanceMap,
    DepthChangeMap,
    TargetSelection,
    MutationRng,
    ExhaustedProposals,
    ZeroDirection,
    EmissionPosition,
    EmissionVisibility,
    NewbornActivation,
    CorpseDirection,
    NumericalFaultPolicy,
    Backend,
    Determinism,
    MathMode,
    CommandDurability,
    ObserverPressure,
    PresentationEngine,
    ColorMapping,
    Interpolation,
    RecordingClock,
    PersistenceCatalog,
    CheckpointMode,
}

impl Role {
    /// The configuration key, as `section.key`.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Boundary => "world.boundary",
            Self::FieldDtype => "world.field_dtype",
            Self::TransportScheme => "world.transport_scheme",
            Self::StabilityPolicy => "world.stability_policy",
            Self::ExchangeKernel => "world.exchange_kernel",
            Self::TinyEnergyPolicy => "world.tiny_energy_policy",
            Self::InitialVelocity => "population.initial_velocity",
            Self::InitialPositions => "population.initial_positions",
            Self::CapacityPolicy => "population.capacity_policy",
            Self::FieldComposition => "field_initialization.composition",
            Self::SourceNormalization => "sources.spatial_normalization",
            Self::SourceKernel => "sources.kernel",
            Self::SourceChemistry => "sources.chemical_directions",
            Self::SourceChemistryPolicy => "sources.chemical_direction_policy",
            Self::SourcePath => "sources.path",
            Self::ChemistryFamily => "chemistry.family",
            Self::HeritableTrait => "chemistry.heritable_traits",
            Self::ChemistryEvaluation => "chemistry.chemistry_evaluation",
            Self::SensorSchema => "sensors.schema",
            Self::SensorNormalization => "sensors.normalization",
            Self::PyramidFilter => "sensors.pyramid_filter",
            Self::PyramidBlend => "sensors.pyramid_scale_blend",
            Self::ControllerFamily => "controller.family",
            Self::HiddenActivation => "controller.hidden_activation",
            Self::OutputActivation => "controller.output_activation",
            Self::WeightDtype => "controller.canonical_weight_dtype",
            Self::AccumulationDtype => "controller.accumulation_dtype",
            Self::PhasePolicy => "controller.phase_policy",
            Self::Initializer => "controller.initializer",
            Self::TopologyCatalog => "architecture_mutation.catalog",
            Self::InheritanceMap => "architecture_mutation.inheritance_map",
            Self::DepthChangeMap => "architecture_mutation.depth_change_map",
            Self::TargetSelection => "architecture_mutation.target_selection",
            Self::MutationRng => "mutation.rng",
            Self::ExhaustedProposals => "mutation.exhausted_proposals_policy",
            Self::ZeroDirection => "emission.zero_direction_policy",
            Self::EmissionPosition => "emission.position",
            Self::EmissionVisibility => "emission.visibility",
            Self::NewbornActivation => "reproduction.newborn_activation",
            Self::CorpseDirection => "death.corpse_direction",
            Self::NumericalFaultPolicy => "death.numerical_fault_policy",
            Self::Backend => "execution.backend",
            Self::Determinism => "execution.determinism",
            Self::MathMode => "execution.math_mode",
            Self::CommandDurability => "execution.command_durability",
            Self::ObserverPressure => "execution.observer_pressure_policy",
            Self::PresentationEngine => "presentation.engine",
            Self::ColorMapping => "presentation.color_mapping",
            Self::Interpolation => "presentation.interpolation",
            Self::RecordingClock => "recording.clock",
            Self::PersistenceCatalog => "persistence.catalog",
            Self::CheckpointMode => "persistence.checkpoint_mode",
        }
    }
}

/// How far a registered definition has been carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Defined in the plan; its owner has not yet implemented it or frozen
    /// its known-answer vectors. A run naming it cannot pass G1 or G3 yet.
    Specified,
    /// Implemented with executable evidence.
    Implemented {
        /// A typed reference to the implementation's evidence.
        evidence: &'static str,
    },
}

/// One registered name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlgorithmEntry {
    /// The versioned name as written in a configuration.
    pub name: &'static str,
    /// The one key it may appear under.
    pub role: Role,
    /// Where the plan defines it.
    pub contract: &'static str,
    /// The work item that implements it.
    pub owner: &'static str,
    /// Its status.
    pub status: Status,
}

const fn entry(
    name: &'static str,
    role: Role,
    contract: &'static str,
    owner: &'static str,
) -> AlgorithmEntry {
    AlgorithmEntry {
        name,
        role,
        contract,
        owner,
        status: Status::Specified,
    }
}

/// Every name a `frankenlife.config.v1` configuration may use.
pub static REGISTRY: &[AlgorithmEntry] = &[
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-core::tests/core_fixtures.rs::appendix_a_starter_validates",
        },
        ..entry("field-ecology-v1", Role::Model, "§4, Appendix A", "FL-002")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-field::tests/field_layout.rs::minimum_image_is_half_open_and_adds_no_rounding",
        },
        ..entry("periodic", Role::Boundary, "§4.1", "FL-003")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-field::tests/field_layout.rs::actual_f32_allocation_at_1024_squared",
        },
        ..entry("f32", Role::FieldDtype, "§4.2, §10.3", "FL-003")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-field::tests/transport_sources.rs::rectangular_stencil_matches_an_independent_oracle_and_conserves",
        },
        ..entry(
            "positive-five-point-v1",
            Role::TransportScheme,
            "§4.5",
            "FL-004",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-field::tests/transport_sources.rs::cfl_bound_subcycles_instead_of_going_negative",
        },
        ..entry("subcycle", Role::StabilityPolicy, "§4.5, §8.4", "FL-004")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-field::tests/uptake_ledger.rs::exchange_footprints_are_bilinear_local_and_periodic",
        },
        ..entry("grid-cic-v1", Role::ExchangeKernel, "§4.3, §5.1", "FL-005")
    },
    // Clearing a tiny reservoir is the last budget stage (§8.2): a
    // reservoir strictly below the threshold is paid in full, recorded as
    // the `tiny-reservoir` flow, and the organism dies of it.
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-engine::tests/lifecycle.rs::tiny_reservoirs_below_the_threshold_are_dissipated_and_recorded",
        },
        ..entry(
            "dissipate-and-record",
            Role::TinyEnergyPolicy,
            "§4.4, §8.2, A.1",
            "FL-012",
        )
    },
    // The ordered tick founds its world (FL-013), so it implements the
    // initial population; FL-015 builds the small-world preset on it.
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-engine::tests/world.rs::founders_follow_appendix_a3_item_1",
        },
        ..entry("zero", Role::InitialVelocity, "A.3 item 1", "FL-013")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-engine::tests/world.rs::founders_follow_appendix_a3_item_1",
        },
        ..entry("uniform", Role::InitialPositions, "A.3 item 1", "FL-013")
    },
    // FL-011's birth stage reports a fixed logical quota shortfall with
    // everything rolled back (`flife_engine::CapacityShortfall`); the
    // ordered tick pauses on it before commit (FL-013).
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-engine::tests/world.rs::a_population_shortfall_pauses_the_tick_and_changes_nothing",
        },
        ..entry(
            "pause-before-commit",
            Role::CapacityPolicy,
            "§7.5",
            "FL-013",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-field::tests/field_layout.rs::appendix_a_initial_field_is_uniform_with_zero_moments",
        },
        ..entry("zero", Role::FieldComposition, "§4.2", "FL-003")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-field::tests/transport_sources.rs::source_totals_do_not_depend_on_resolution_or_position",
        },
        ..entry(
            "fixed-total-power",
            Role::SourceNormalization,
            "§4.3, A.3 item 9",
            "FL-004",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-field::tests/transport_sources.rs::small_supports_deposit_exact_totals",
        },
        ..entry(
            "normalized-truncated-minimum-image-gaussian-v1",
            Role::SourceKernel,
            "A.3 item 9",
            "FL-004",
        )
    },
    // FL-004 implemented the arithmetic; FL-010 supplies the semantic draws
    // (`flife_field::seeding`).
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-field::tests/seeding.rs::seeded_directions_match_an_independent_oracle",
        },
        ..entry(
            "seeded-unit-vectors",
            Role::SourceChemistry,
            "A.3 item 7",
            "FL-004",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-field::tests/transport_sources.rs::chemical_directions_stay_inside_the_unit_ball",
        },
        ..entry(
            "fixed-per-source",
            Role::SourceChemistryPolicy,
            "A.3 item 7",
            "FL-004",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-field::tests/seeding.rs::seeded_paths_chain_their_own_directions_and_join_continuously",
        },
        ..entry(
            "seeded-smooth-torus-v1",
            Role::SourcePath,
            "A.3 item 8",
            "FL-004",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-engine::tests/organism_phase.rs::c9_chemistry_is_evaluated_per_packet_not_on_the_mixture",
        },
        ..entry(
            "bounded-affinity-v1",
            Role::ChemistryFamily,
            "§5.3",
            "FL-006",
        )
    },
    // FL-006 implements these traits' representation, norm bound and use
    // (`flife_engine::Affinity`, `flife_engine::MotionResponse`), FL-007
    // sensing at any admitted scale, and FL-010 their founders' draws and
    // mutation as A.3 item 4 specifies (`flife_genome::mutation`).
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-genome::tests/mutation.rs::each_operator_changes_only_its_own_heritable_component",
        },
        ..entry(
            "affinity_direction",
            Role::HeritableTrait,
            "§5.3, A.3 item 4",
            "FL-010",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-genome::tests/mutation.rs::each_operator_changes_only_its_own_heritable_component",
        },
        ..entry(
            "motion_response",
            Role::HeritableTrait,
            "§5.4, A.3 item 4",
            "FL-010",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-genome::tests/mutation.rs::sensing_scale_reflects_at_its_log_bounds",
        },
        ..entry(
            "sensing_scale",
            Role::HeritableTrait,
            "§6.1, A.3 item 4",
            "FL-010",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-engine::tests/organism_phase.rs::organisms_settle_contended_uptake_and_close_the_ledger_every_tick",
        },
        ..entry(
            "per-granted-cell-packet",
            Role::ChemistryEvaluation,
            "§5.3",
            "FL-006",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-neural::tests/sensing.rs::observations_follow_the_abi_order_and_formulas",
        },
        ..entry(
            "energy-moment-gradient-104-v1",
            Role::SensorSchema,
            "§6.1",
            "FL-007",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-neural::tests/sensing.rs::observations_follow_the_abi_order_and_formulas",
        },
        ..entry(
            "fixed-local-scales-v1",
            Role::SensorNormalization,
            "§6.1, A.1",
            "FL-007",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-neural::tests/sensing.rs::pyramid_matches_a_slow_direct_oracle",
        },
        ..entry("positive-box-v1", Role::PyramidFilter, "§6.1", "FL-007")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-neural::tests/sensing.rs::knots_take_the_right_hand_interval_and_edges_are_interior",
        },
        ..entry("log-linear-v1", Role::PyramidBlend, "§6.1", "FL-007")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-neural::tests/controller.rs::hand_set_network_matches_independent_known_answers",
        },
        ..entry(
            "independent-dense-mlp-v1",
            Role::ControllerFamily,
            "§6.2, §6.5",
            "FL-008",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-neural::tests/controller.rs::c16_a_tanh_layer_is_not_an_identity",
        },
        ..entry("tanh", Role::HiddenActivation, "§6.2", "FL-008")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-neural::tests/controller.rs::c11_zero_weights_pass_output_biases_through_a_linear_layer",
        },
        ..entry("linear", Role::OutputActivation, "§6.3", "FL-008")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-genome::tests/genomes.rs::genomes_are_sealed_once_and_shared_read_only",
        },
        ..entry("f32", Role::WeightDtype, "§10.3", "FL-009")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-neural::tests/controller.rs::accumulation_is_f32_bias_first_in_ascending_input_order",
        },
        ..entry("f32", Role::AccumulationDtype, "§10.3", "FL-008")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-core::tests/core_fixtures.rs::stable_phase_is_identity_mod_interval",
        },
        ..entry(
            "stable-identity",
            Role::PhasePolicy,
            "A.3 item 10",
            "FL-002",
        )
    },
    // FL-009 implements the map from raw words to parameters
    // (`flife_genome::init`); FL-010 the founders' semantic words
    // (`flife_genome::founder`).
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-genome::tests/mutation.rs::founders_are_functions_of_the_seed_and_their_identity",
        },
        ..entry(
            "fan-in-scaled-symmetric-v1",
            Role::Initializer,
            "§7.2, A.3 item 2",
            "FL-010",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-genome::tests/genomes.rs::catalog_is_section_6_4_in_order_under_the_parameter_cap",
        },
        ..entry(
            "dense-104-36-catalog-v1",
            Role::TopologyCatalog,
            "§6.4",
            "FL-009",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-genome::tests/genomes.rs::equal_depth_widening_preserves_the_parent_function",
        },
        ..entry(
            "overlap-plus-zero-output-v1",
            Role::InheritanceMap,
            "A.3 item 5",
            "FL-009",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-genome::tests/genomes.rs::deepening_inserts_an_identity_bridge",
        },
        ..entry(
            "prefix-and-identity-bridge-v1",
            Role::DepthChangeMap,
            "A.3 item 6",
            "FL-009",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-genome::tests/mutation.rs::topology_changes_choose_uniformly_among_the_other_admitted_members",
        },
        ..entry(
            "uniform-other-catalog-member",
            Role::TargetSelection,
            "A.3 item 4",
            "FL-010",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-core::tests/rng.rs::semantic_blocks_match_numpy",
        },
        ..entry(
            "fnp-philox-semantic-v1",
            Role::MutationRng,
            "§16.2, A.3 item 3",
            "FL-010",
        )
    },
    // Rejection, abandonment and the record are FL-010's. The birth
    // transaction (FL-011) keeps the parent's energy and cooldown and issues
    // no identity for a rejected birth
    // (`flife-engine::tests/emission_births.rs::rejected_births_charge_nothing_and_take_no_identity`).
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-genome::tests/mutation.rs::c16_exhausted_no_op_candidates_reject_the_birth_without_charge",
        },
        ..entry(
            "reject-birth-no-charge",
            Role::ExhaustedProposals,
            "A.3 item 4, C.16",
            "FL-010",
        )
    },
    // FL-007 decodes a closed gate or a zero direction as no proposal;
    // FL-011 charges nothing for it (`flife_engine::emission`).
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-engine::tests/emission_births.rs::no_emission_no_charge_for_closed_gates_zero_directions_and_the_dead",
        },
        ..entry(
            "no-emission-no-charge",
            Role::ZeroDirection,
            "§5.5",
            "FL-011",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-engine::tests/emission_births.rs::emission_spends_the_decoded_fraction_of_the_post_motion_budget",
        },
        ..entry("post-motion", Role::EmissionPosition, "§5.5", "FL-011")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-engine::tests/emission_births.rs::deposits_are_first_sensed_and_absorbed_on_the_next_tick",
        },
        ..entry("next-tick", Role::EmissionVisibility, "§5.5", "FL-011")
    },
    // The birth transaction stamps newborns with birth tick `n + 1`, an
    // invalid held output and zero history; the ordered tick (FL-013) first
    // advances them from that committed state.
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-engine::tests/emission_births.rs::newborns_start_inert_and_join_on_the_next_tick",
        },
        ..entry("next-tick", Role::NewbornActivation, "§6.6, §7.4", "FL-011")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-engine::tests/lifecycle.rs::recycling_returns_the_configured_fraction_with_the_genetic_affinity",
        },
        ..entry("genetic-affinity", Role::CorpseDirection, "§7.6", "FL-012")
    },
    // FL-012 refuses a numerical fault by identity and never collects it as
    // a death (`flife_engine::Population`); the ordered tick stops the run
    // with the committed state preserved (FL-013).
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-engine::tests/world.rs::a_numerical_fault_stops_the_run_and_preserves_the_committed_state",
        },
        ..entry(
            "stop-and-preserve",
            Role::NumericalFaultPolicy,
            "§7.6",
            "FL-013",
        )
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-engine::tests/world.rs::a_small_headless_ecology_lives_breeds_mutates_emits_and_dies",
        },
        ..entry("cpu-reference", Role::Backend, "§10.1, §18.1", "FL-013")
    },
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-engine::tests/world.rs::c7_compact_and_free_list_worlds_publish_the_same_epochs",
        },
        ..entry("qualified-reference", Role::Determinism, "§16.1", "FL-013")
    },
    entry("specified", Role::MathMode, "§10.3, A.1", "FL-008"),
    entry(
        "write-ahead-before-apply",
        Role::CommandDurability,
        "§12.3",
        "FL-036",
    ),
    // §12.3's labelled best-effort mode: the session owner holds acceptance
    // in memory and acknowledges it as volatile, lost if the process ends
    // (FL-014). The write-ahead journal above is FL-036's.
    AlgorithmEntry {
        status: Status::Implemented {
            evidence: "test:flife-runtime::tests/session.rs::the_session_admits_volatile_in_memory_and_refuses_write_ahead_until_the_journal",
        },
        ..entry(
            "volatile-in-memory",
            Role::CommandDurability,
            "§12.3",
            "FL-014",
        )
    },
    entry(
        "drop-observer-or-pause-unchanged-world",
        Role::ObserverPressure,
        "§7.5, §9.5",
        "FL-017",
    ),
    entry("franken-manim", Role::PresentationEngine, "§13.1", "FL-018"),
    entry("founder-lineage-v1", Role::ColorMapping, "§13.4", "FL-019"),
    entry(
        "committed-states-one-epoch-late",
        Role::Interpolation,
        "§13.5, C.15",
        "FL-020",
    ),
    entry(
        "simulation-time",
        Role::RecordingClock,
        "§12.2, §13.8",
        "FL-023",
    ),
    entry("frankensqlite", Role::PersistenceCatalog, "§16.6", "FL-036"),
    entry(
        "pause-and-stream",
        Role::CheckpointMode,
        "§16.5, A.2",
        "FL-037",
    ),
];

/// Resolve `name` for `role`.
///
/// # Errors
/// [`CoreError::UnknownAlgorithm`] when no entry has this name and role.
pub fn lookup(role: Role, name: &str) -> Result<&'static AlgorithmEntry, CoreError> {
    REGISTRY
        .iter()
        .find(|entry| entry.role == role && entry.name == name)
        .ok_or_else(|| CoreError::UnknownAlgorithm {
            role: role.key(),
            name: name.to_owned(),
        })
}

/// Bytes per element of a registered dtype name.
#[must_use]
pub fn dtype_bytes(name: &str) -> Option<u64> {
    match name {
        "f32" => Some(4),
        _ => None,
    }
}
