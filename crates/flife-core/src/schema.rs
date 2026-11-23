//! The configuration schema table: one row per key.
//!
//! Each row gives a key's value, type, unit, generic bound and, for a named
//! algorithm, its registry role. The canonical writer, the generic
//! validator and the model descriptor all read this one table, so they
//! cannot disagree. A struct field missing here makes the canonical output
//! unparseable (every key is required), and the round-trip test catches it.

use crate::config::RunConfig;
use crate::registry::Role;
use crate::units::Unit;

/// A generic bound on a float.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatBound {
    /// Finite.
    Finite,
    /// Finite and `>= 0`.
    NonNegative,
    /// Finite and `> 0`.
    Positive,
    /// In `[0, 1]`.
    UnitClosed,
    /// In `(0, 1]`.
    UnitPositive,
}

/// A generic bound on an integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntBound {
    /// Any value TOML can represent (`<= i64::MAX`).
    Any,
    /// At least 1.
    AtLeastOne,
    /// At least 2: a render queue needs one displayable epoch and one
    /// being written (§9.5).
    AtLeastTwo,
}

/// What kind of value a key holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A float with a unit and a bound.
    Float(Unit, FloatBound),
    /// An integer with a unit and a bound.
    Int(Unit, IntBound),
    /// A boolean switch.
    Bool,
    /// Free text with a per-key format rule.
    Text,
    /// A registered name for this role.
    Name(Role),
    /// A list of registered names for this role.
    NameList(Role),
    /// A list of integers with a unit.
    IntList(Unit),
}

/// A key's value, borrowed from the configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value<'a> {
    /// A float.
    Float(f64),
    /// An integer.
    Int(u64),
    /// A boolean.
    Bool(bool),
    /// A string (text or a registered name).
    Str(&'a str),
    /// A list of strings.
    StrList(&'a [String]),
    /// A list of integers.
    IntList(&'a [u32]),
}

/// Which identity a key belongs to (plan B.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// Run identity (schema, name, seed); recorded, not part of the model.
    Run,
    /// The scientific model; part of the model descriptor.
    Model,
    /// The numerical profile; part of the model descriptor.
    Numerics,
    /// Execution resources and policies; not part of the model.
    Execution,
    /// Display, recording and persistence; not part of the model.
    Presentation,
}

/// One schema row bound to a configuration's value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field<'a> {
    /// The TOML table, or `""` for top-level keys.
    pub section: &'static str,
    /// The key within its table.
    pub key: &'static str,
    /// The value.
    pub value: Value<'a>,
    /// Its type, unit and bound.
    pub kind: Kind,
    /// The identity it belongs to.
    pub scope: Scope,
}

impl Field<'_> {
    /// `section.key`, or `key` at top level.
    #[must_use]
    pub fn path(&self) -> String {
        if self.section.is_empty() {
            self.key.to_owned()
        } else {
            format!("{}.{}", self.section, self.key)
        }
    }
}

fn scope_of(section: &str, key: &str) -> Scope {
    match (section, key) {
        ("", "model") => Scope::Model,
        ("", _) => Scope::Run,
        ("execution", "determinism" | "math_mode") => Scope::Numerics,
        ("execution", _) => Scope::Execution,
        ("presentation" | "recording" | "persistence" | "v2.visual", _) => Scope::Presentation,
        _ => Scope::Model,
    }
}

struct Rows<'a>(Vec<Field<'a>>);

impl<'a> Rows<'a> {
    fn push(&mut self, section: &'static str, key: &'static str, value: Value<'a>, kind: Kind) {
        self.0.push(Field {
            section,
            key,
            value,
            kind,
            scope: scope_of(section, key),
        });
    }
    fn float(&mut self, s: &'static str, k: &'static str, v: f64, unit: Unit, bound: FloatBound) {
        self.push(s, k, Value::Float(v), Kind::Float(unit, bound));
    }
    fn int(
        &mut self,
        s: &'static str,
        k: &'static str,
        v: impl Into<u64>,
        unit: Unit,
        bound: IntBound,
    ) {
        self.push(s, k, Value::Int(v.into()), Kind::Int(unit, bound));
    }
    fn boolean(&mut self, s: &'static str, k: &'static str, v: bool) {
        self.push(s, k, Value::Bool(v), Kind::Bool);
    }
    fn text(&mut self, s: &'static str, k: &'static str, v: &'a str) {
        self.push(s, k, Value::Str(v), Kind::Text);
    }
    fn name(&mut self, s: &'static str, k: &'static str, v: &'a str, role: Role) {
        self.push(s, k, Value::Str(v), Kind::Name(role));
    }
}

/// Every key of `config`, in canonical (Appendix A) order.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one row per Appendix A key, kept in one auditable table"
)]
pub fn fields(config: &RunConfig) -> Vec<Field<'_>> {
    use FloatBound::{NonNegative, Positive, UnitClosed, UnitPositive};
    use IntBound::{Any, AtLeastOne};
    use Unit as U;

    let mut r = Rows(Vec::with_capacity(160));
    r.text("", "schema", &config.schema);
    r.text("", "name", &config.name);
    r.name("", "model", &config.model, Role::Model);
    r.int("", "seed", config.seed, U::Identifier, Any);

    let w = &config.world;
    r.float("world", "width", w.width, U::WorldLength, Positive);
    r.float("world", "height", w.height, U::WorldLength, Positive);
    r.int("world", "grid_width", w.grid_width, U::Count, AtLeastOne);
    r.int("world", "grid_height", w.grid_height, U::Count, AtLeastOne);
    r.name("world", "boundary", &w.boundary, Role::Boundary);
    r.int(
        "world",
        "moment_channels",
        w.moment_channels,
        U::Count,
        AtLeastOne,
    );
    r.name("world", "field_dtype", &w.field_dtype, Role::FieldDtype);
    r.int(
        "world",
        "tick_numerator",
        w.tick_numerator,
        U::TickRationalPart,
        AtLeastOne,
    );
    r.int(
        "world",
        "tick_denominator",
        w.tick_denominator,
        U::TickRationalPart,
        AtLeastOne,
    );
    r.name(
        "world",
        "transport_scheme",
        &w.transport_scheme,
        Role::TransportScheme,
    );
    r.float("world", "diffusion", w.diffusion, U::Diffusion, NonNegative);
    r.float(
        "world",
        "decay_per_second",
        w.decay_per_second,
        U::PerSimulationSecond,
        NonNegative,
    );
    r.name(
        "world",
        "stability_policy",
        &w.stability_policy,
        Role::StabilityPolicy,
    );
    r.name(
        "world",
        "exchange_kernel",
        &w.exchange_kernel,
        Role::ExchangeKernel,
    );
    if let Some(radius) = w.exchange_radius_world_units {
        r.float(
            "world",
            "exchange_radius_world_units",
            radius,
            U::WorldLength,
            Positive,
        );
    }
    r.name(
        "world",
        "tiny_energy_policy",
        &w.tiny_energy_policy,
        Role::TinyEnergyPolicy,
    );
    r.float(
        "world",
        "tiny_energy_threshold",
        w.tiny_energy_threshold,
        U::Energy,
        NonNegative,
    );

    let p = &config.population;
    r.int(
        "population",
        "initial_count",
        p.initial_count,
        U::Count,
        Any,
    );
    r.int(
        "population",
        "maximum_count",
        p.maximum_count,
        U::Count,
        AtLeastOne,
    );
    r.float(
        "population",
        "initial_energy",
        p.initial_energy,
        U::Energy,
        Positive,
    );
    r.name(
        "population",
        "initial_velocity",
        &p.initial_velocity,
        Role::InitialVelocity,
    );
    r.name(
        "population",
        "initial_positions",
        &p.initial_positions,
        Role::InitialPositions,
    );
    r.name(
        "population",
        "capacity_policy",
        &p.capacity_policy,
        Role::CapacityPolicy,
    );
    r.boolean("population", "automatic_reseed", p.automatic_reseed);

    let f = &config.field_initialization;
    r.float(
        "field_initialization",
        "energy_per_cell",
        f.energy_per_cell,
        U::Energy,
        NonNegative,
    );
    r.name(
        "field_initialization",
        "composition",
        &f.composition,
        Role::FieldComposition,
    );

    let s = &config.sources;
    r.int("sources", "count", s.count, U::Count, Any);
    r.float(
        "sources",
        "power_per_source",
        s.power_per_source,
        U::EnergyPerSecond,
        Positive,
    );
    r.float(
        "sources",
        "sigma_world_units",
        s.sigma_world_units,
        U::WorldLength,
        Positive,
    );
    r.name(
        "sources",
        "spatial_normalization",
        &s.spatial_normalization,
        Role::SourceNormalization,
    );
    r.name("sources", "kernel", &s.kernel, Role::SourceKernel);
    r.float(
        "sources",
        "kernel_cutoff_sigma",
        s.kernel_cutoff_sigma,
        U::Dimensionless,
        Positive,
    );
    r.name(
        "sources",
        "chemical_directions",
        &s.chemical_directions,
        Role::SourceChemistry,
    );
    r.name(
        "sources",
        "chemical_direction_policy",
        &s.chemical_direction_policy,
        Role::SourceChemistryPolicy,
    );
    r.name("sources", "path", &s.path, Role::SourcePath);
    r.float(
        "sources",
        "path_speed",
        s.path_speed,
        U::WorldLengthPerSecond,
        NonNegative,
    );
    r.float(
        "sources",
        "path_direction_interval_seconds",
        s.path_direction_interval_seconds,
        U::SimulationSeconds,
        Positive,
    );

    let c = &config.chemistry;
    r.name("chemistry", "family", &c.family, Role::ChemistryFamily);
    r.push(
        "chemistry",
        "heritable_traits",
        Value::StrList(&c.heritable_traits),
        Kind::NameList(Role::HeritableTrait),
    );
    r.float(
        "chemistry",
        "maximum_efficiency",
        c.maximum_efficiency,
        U::Dimensionless,
        UnitPositive,
    );
    r.float(
        "chemistry",
        "affinity_norm_limit",
        c.affinity_norm_limit,
        U::Dimensionless,
        Positive,
    );
    r.float(
        "chemistry",
        "affinity_gain",
        c.affinity_gain,
        U::Dimensionless,
        NonNegative,
    );
    r.float(
        "chemistry",
        "discrimination_threshold",
        c.discrimination_threshold,
        U::Dimensionless,
        Positive,
    );
    r.float(
        "chemistry",
        "uptake_capacity_per_second",
        c.uptake_capacity_per_second,
        U::EnergyPerSecond,
        NonNegative,
    );
    r.float(
        "chemistry",
        "uptake_half_saturation_density",
        c.uptake_half_saturation_density,
        U::EnergyDensity,
        Positive,
    );
    r.float(
        "chemistry",
        "toxicity_sensitivity",
        c.toxicity_sensitivity,
        U::Dimensionless,
        Positive,
    );
    r.name(
        "chemistry",
        "chemistry_evaluation",
        &c.chemistry_evaluation,
        Role::ChemistryEvaluation,
    );
    r.float(
        "chemistry",
        "motion_response_norm_limit",
        c.motion_response_norm_limit,
        U::Dimensionless,
        Positive,
    );
    r.float(
        "chemistry",
        "maximum_acceleration",
        c.maximum_acceleration,
        U::WorldLengthPerSecondSquared,
        Positive,
    );
    r.float(
        "chemistry",
        "drag_per_second",
        c.drag_per_second,
        U::PerSimulationSecond,
        NonNegative,
    );
    r.float(
        "chemistry",
        "maximum_speed",
        c.maximum_speed,
        U::WorldLengthPerSecond,
        Positive,
    );

    let n = &config.sensors;
    r.name("sensors", "schema", &n.schema, Role::SensorSchema);
    r.name(
        "sensors",
        "normalization",
        &n.normalization,
        Role::SensorNormalization,
    );
    r.float(
        "sensors",
        "energy_density_scale",
        n.energy_density_scale,
        U::EnergyDensity,
        Positive,
    );
    r.float(
        "sensors",
        "gradient_length_scale_world_units",
        n.gradient_length_scale_world_units,
        U::WorldLength,
        Positive,
    );
    r.float(
        "sensors",
        "composition_energy_floor_density",
        n.composition_energy_floor_density,
        U::EnergyDensity,
        Positive,
    );
    r.float(
        "sensors",
        "internal_energy_scale",
        n.internal_energy_scale,
        U::Energy,
        Positive,
    );
    r.float(
        "sensors",
        "age_scale_seconds",
        n.age_scale_seconds,
        U::SimulationSeconds,
        Positive,
    );
    r.float(
        "sensors",
        "velocity_scale",
        n.velocity_scale,
        U::WorldLengthPerSecond,
        Positive,
    );
    r.float(
        "sensors",
        "energy_change_scale_per_second",
        n.energy_change_scale_per_second,
        U::EnergyPerSecond,
        Positive,
    );
    r.float(
        "sensors",
        "initial_scale_world_units",
        n.initial_scale_world_units,
        U::WorldLength,
        Positive,
    );
    r.float(
        "sensors",
        "minimum_scale_world_units",
        n.minimum_scale_world_units,
        U::WorldLength,
        Positive,
    );
    r.float(
        "sensors",
        "maximum_scale_world_units",
        n.maximum_scale_world_units,
        U::WorldLength,
        Positive,
    );
    r.name(
        "sensors",
        "pyramid_filter",
        &n.pyramid_filter,
        Role::PyramidFilter,
    );
    r.name(
        "sensors",
        "pyramid_scale_blend",
        &n.pyramid_scale_blend,
        Role::PyramidBlend,
    );

    let k = &config.controller;
    r.name("controller", "family", &k.family, Role::ControllerFamily);
    if let Some(width) = k.recurrent_state_width {
        r.int(
            "controller",
            "recurrent_state_width",
            width,
            U::Count,
            AtLeastOne,
        );
    }
    r.int(
        "controller",
        "input_width",
        k.input_width,
        U::Count,
        AtLeastOne,
    );
    r.push(
        "controller",
        "hidden_widths",
        Value::IntList(&k.hidden_widths),
        Kind::IntList(U::Count),
    );
    r.int(
        "controller",
        "output_width",
        k.output_width,
        U::Count,
        AtLeastOne,
    );
    r.name(
        "controller",
        "hidden_activation",
        &k.hidden_activation,
        Role::HiddenActivation,
    );
    r.name(
        "controller",
        "output_activation",
        &k.output_activation,
        Role::OutputActivation,
    );
    r.name(
        "controller",
        "canonical_weight_dtype",
        &k.canonical_weight_dtype,
        Role::WeightDtype,
    );
    r.name(
        "controller",
        "accumulation_dtype",
        &k.accumulation_dtype,
        Role::AccumulationDtype,
    );
    r.int(
        "controller",
        "evaluation_interval_ticks",
        k.evaluation_interval_ticks,
        U::Ticks,
        AtLeastOne,
    );
    r.name(
        "controller",
        "phase_policy",
        &k.phase_policy,
        Role::PhasePolicy,
    );
    r.boolean("controller", "autograd", k.autograd);
    r.name(
        "controller",
        "initializer",
        &k.initializer,
        Role::Initializer,
    );
    r.float(
        "controller",
        "maximum_absolute_parameter",
        k.maximum_absolute_parameter,
        U::Dimensionless,
        Positive,
    );

    let a = &config.architecture_mutation;
    r.boolean("architecture_mutation", "enabled", a.enabled);
    r.name(
        "architecture_mutation",
        "catalog",
        &a.catalog,
        Role::TopologyCatalog,
    );
    r.int(
        "architecture_mutation",
        "maximum_parameters",
        a.maximum_parameters,
        U::Count,
        AtLeastOne,
    );
    r.float(
        "architecture_mutation",
        "probability_per_birth",
        a.probability_per_birth,
        U::Probability,
        UnitClosed,
    );
    r.name(
        "architecture_mutation",
        "inheritance_map",
        &a.inheritance_map,
        Role::InheritanceMap,
    );
    r.name(
        "architecture_mutation",
        "depth_change_map",
        &a.depth_change_map,
        Role::DepthChangeMap,
    );
    r.name(
        "architecture_mutation",
        "target_selection",
        &a.target_selection,
        Role::TargetSelection,
    );

    let m = &config.mutation;
    r.name("mutation", "rng", &m.rng, Role::MutationRng);
    r.float(
        "mutation",
        "weight_probability",
        m.weight_probability,
        U::Probability,
        UnitClosed,
    );
    r.float(
        "mutation",
        "weight_standard_deviation",
        m.weight_standard_deviation,
        U::Dimensionless,
        NonNegative,
    );
    r.float(
        "mutation",
        "replacement_probability",
        m.replacement_probability,
        U::Probability,
        UnitClosed,
    );
    r.float(
        "mutation",
        "chemistry_standard_deviation",
        m.chemistry_standard_deviation,
        U::Dimensionless,
        NonNegative,
    );
    r.float(
        "mutation",
        "sensing_log_scale_standard_deviation",
        m.sensing_log_scale_standard_deviation,
        U::Dimensionless,
        NonNegative,
    );
    r.boolean("mutation", "reject_effective_noop", m.reject_effective_noop);
    r.int(
        "mutation",
        "maximum_candidate_proposals_per_birth",
        m.maximum_candidate_proposals_per_birth,
        U::Count,
        AtLeastOne,
    );
    r.name(
        "mutation",
        "exhausted_proposals_policy",
        &m.exhausted_proposals_policy,
        Role::ExhaustedProposals,
    );
    r.boolean("mutation", "meta_evolution", m.meta_evolution);

    let o = &config.costs;
    r.float(
        "costs",
        "basal_energy_per_second",
        o.basal_energy_per_second,
        U::EnergyPerSecond,
        NonNegative,
    );
    r.float(
        "costs",
        "sensor_energy_per_second_at_unit_scale",
        o.sensor_energy_per_second_at_unit_scale,
        U::EnergyPerSecond,
        NonNegative,
    );
    r.float(
        "costs",
        "sensor_unit_scale_world_units",
        o.sensor_unit_scale_world_units,
        U::WorldLength,
        Positive,
    );
    r.float(
        "costs",
        "sensor_scale_cost_exponent",
        o.sensor_scale_cost_exponent,
        U::Dimensionless,
        NonNegative,
    );
    r.float(
        "costs",
        "neural_energy_per_parameter_evaluation",
        o.neural_energy_per_parameter_evaluation,
        U::EnergyPerParameterEvaluation,
        NonNegative,
    );
    r.float(
        "costs",
        "motion_energy_per_speed_squared_second",
        o.motion_energy_per_speed_squared_second,
        U::EnergySecondsPerLengthSquared,
        NonNegative,
    );
    r.float(
        "costs",
        "birth_energy",
        o.birth_energy,
        U::Energy,
        NonNegative,
    );

    let e = &config.emission;
    r.float(
        "emission",
        "gate_threshold",
        e.gate_threshold,
        U::Probability,
        UnitClosed,
    );
    r.float(
        "emission",
        "maximum_fractional_rate_per_second",
        e.maximum_fractional_rate_per_second,
        U::PerSimulationSecond,
        NonNegative,
    );
    r.float(
        "emission",
        "transfer_efficiency",
        e.transfer_efficiency,
        U::Dimensionless,
        UnitPositive,
    );
    r.float(
        "emission",
        "direction_zero_threshold",
        e.direction_zero_threshold,
        U::Dimensionless,
        NonNegative,
    );
    r.name(
        "emission",
        "zero_direction_policy",
        &e.zero_direction_policy,
        Role::ZeroDirection,
    );
    r.name("emission", "position", &e.position, Role::EmissionPosition);
    r.name(
        "emission",
        "visibility",
        &e.visibility,
        Role::EmissionVisibility,
    );

    let b = &config.reproduction;
    r.float(
        "reproduction",
        "maximum_attempt_rate_per_second",
        b.maximum_attempt_rate_per_second,
        U::PerSimulationSecond,
        NonNegative,
    );
    r.float(
        "reproduction",
        "minimum_parent_energy_before_attempt",
        b.minimum_parent_energy_before_attempt,
        U::Energy,
        NonNegative,
    );
    r.float(
        "reproduction",
        "minimum_child_energy",
        b.minimum_child_energy,
        U::Energy,
        NonNegative,
    );
    r.float(
        "reproduction",
        "minimum_parent_energy_after_split",
        b.minimum_parent_energy_after_split,
        U::Energy,
        NonNegative,
    );
    r.float(
        "reproduction",
        "minimum_child_fraction",
        b.minimum_child_fraction,
        U::Dimensionless,
        UnitClosed,
    );
    r.float(
        "reproduction",
        "maximum_child_fraction",
        b.maximum_child_fraction,
        U::Dimensionless,
        UnitClosed,
    );
    r.float(
        "reproduction",
        "cooldown_seconds",
        b.cooldown_seconds,
        U::SimulationSeconds,
        NonNegative,
    );
    r.float(
        "reproduction",
        "child_position_offset_radius",
        b.child_position_offset_radius,
        U::WorldLength,
        NonNegative,
    );
    r.name(
        "reproduction",
        "newborn_activation",
        &b.newborn_activation,
        Role::NewbornActivation,
    );

    let d = &config.death;
    r.float(
        "death",
        "residual_energy_recycle_fraction",
        d.residual_energy_recycle_fraction,
        U::Dimensionless,
        UnitClosed,
    );
    r.name(
        "death",
        "corpse_direction",
        &d.corpse_direction,
        Role::CorpseDirection,
    );
    r.name(
        "death",
        "numerical_fault_policy",
        &d.numerical_fault_policy,
        Role::NumericalFaultPolicy,
    );

    if let Some(c) = &config.contact {
        r.float("contact", "diameter", c.diameter, U::WorldLength, Positive);
        r.float(
            "contact",
            "stiffness",
            c.stiffness,
            U::Dimensionless,
            UnitPositive,
        );
        r.float(
            "contact",
            "adhesion",
            c.adhesion,
            U::Dimensionless,
            UnitClosed,
        );
        r.float(
            "contact",
            "adhesion_range",
            c.adhesion_range,
            U::Dimensionless,
            Positive,
        );
        if let Some(sweeps) = c.sweeps {
            r.int("contact", "sweeps", sweeps, U::Count, AtLeastOne);
        }
        if let Some(inhibition) = c.inhibition {
            r.float(
                "contact",
                "inhibition",
                inhibition,
                U::Dimensionless,
                UnitClosed,
            );
        }
        if let Some(repulsion) = c.repulsion {
            r.float(
                "contact",
                "repulsion",
                repulsion,
                U::Dimensionless,
                UnitClosed,
            );
        }
    }

    let x = &config.execution;
    r.name("execution", "backend", &x.backend, Role::Backend);
    r.name(
        "execution",
        "determinism",
        &x.determinism,
        Role::Determinism,
    );
    r.name("execution", "math_mode", &x.math_mode, Role::MathMode);
    r.int(
        "execution",
        "cpu_workers",
        x.cpu_workers,
        U::Count,
        AtLeastOne,
    );
    r.int(
        "execution",
        "command_queue_capacity",
        x.command_queue_capacity,
        U::Count,
        AtLeastOne,
    );
    r.int(
        "execution",
        "control_queue_capacity",
        x.control_queue_capacity,
        U::Count,
        AtLeastOne,
    );
    r.name(
        "execution",
        "command_durability",
        &x.command_durability,
        Role::CommandDurability,
    );
    r.int(
        "execution",
        "render_queue_capacity",
        x.render_queue_capacity,
        U::Count,
        IntBound::AtLeastTwo,
    );
    r.int(
        "execution",
        "maximum_logical_genome_bytes",
        x.maximum_logical_genome_bytes,
        U::Bytes,
        AtLeastOne,
    );
    r.int(
        "execution",
        "maximum_allocated_genome_bytes",
        x.maximum_allocated_genome_bytes,
        U::Bytes,
        AtLeastOne,
    );
    r.int(
        "execution",
        "maximum_total_bytes",
        x.maximum_total_bytes,
        U::Bytes,
        AtLeastOne,
    );
    r.int(
        "execution",
        "checkpoint_retained_bytes",
        x.checkpoint_retained_bytes,
        U::Bytes,
        Any,
    );
    r.name(
        "execution",
        "observer_pressure_policy",
        &x.observer_pressure_policy,
        Role::ObserverPressure,
    );

    let v = &config.presentation;
    r.name(
        "presentation",
        "engine",
        &v.engine,
        Role::PresentationEngine,
    );
    r.int(
        "presentation",
        "width",
        v.width,
        U::DisplayPixels,
        AtLeastOne,
    );
    r.int(
        "presentation",
        "height",
        v.height,
        U::DisplayPixels,
        AtLeastOne,
    );
    r.int(
        "presentation",
        "frames_per_second",
        v.frames_per_second,
        U::DisplayFramesPerSecond,
        AtLeastOne,
    );
    r.name(
        "presentation",
        "color_mapping",
        &v.color_mapping,
        Role::ColorMapping,
    );
    r.text("presentation", "background", &v.background);
    r.float(
        "presentation",
        "point_radius_pixels",
        v.point_radius_pixels,
        U::DisplayPixels,
        Positive,
    );
    r.boolean("presentation", "bloom_enabled", v.bloom_enabled);
    r.float(
        "presentation",
        "bloom_strength",
        v.bloom_strength,
        U::Dimensionless,
        UnitClosed,
    );
    r.boolean("presentation", "trails_enabled", v.trails_enabled);
    r.name(
        "presentation",
        "interpolation",
        &v.interpolation,
        Role::Interpolation,
    );
    r.boolean(
        "presentation",
        "show_clock_telemetry",
        v.show_clock_telemetry,
    );

    let g = &config.recording;
    r.boolean("recording", "enabled", g.enabled);
    r.name("recording", "clock", &g.clock, Role::RecordingClock);
    r.boolean("recording", "include_provenance", g.include_provenance);

    let q = &config.persistence;
    r.name(
        "persistence",
        "catalog",
        &q.catalog,
        Role::PersistenceCatalog,
    );
    r.name(
        "persistence",
        "checkpoint_mode",
        &q.checkpoint_mode,
        Role::CheckpointMode,
    );
    r.float(
        "persistence",
        "checkpoint_interval_simulation_seconds",
        q.checkpoint_interval_simulation_seconds,
        U::SimulationSeconds,
        Positive,
    );
    r.int(
        "persistence",
        "retain_recent_checkpoints",
        q.retain_recent_checkpoints,
        U::Count,
        AtLeastOne,
    );
    r.boolean(
        "persistence",
        "preserve_pinned_discoveries",
        q.preserve_pinned_discoveries,
    );
    r.int(
        "persistence",
        "metrics_interval_ticks",
        q.metrics_interval_ticks,
        U::Ticks,
        AtLeastOne,
    );

    if let Some(v2) = &config.v2 {
        r.text("v2", "profile", &v2.profile);

        if let Some(enabled) = v2.oases.enabled {
            r.boolean("v2.oases", "enabled", enabled);
        }
        r.float(
            "v2.oases",
            "bloom_period_seconds",
            v2.oases.bloom_period_seconds,
            U::SimulationSeconds,
            Positive,
        );
        r.float(
            "v2.oases",
            "minimum_power_fraction",
            v2.oases.minimum_power_fraction,
            U::Dimensionless,
            UnitClosed,
        );
        if let Some(s_min) = v2.oases.sigma_min_world_units {
            r.float(
                "v2.oases",
                "sigma_min_world_units",
                s_min,
                U::WorldLength,
                Positive,
            );
        }
        if let Some(s_max) = v2.oases.sigma_max_world_units {
            r.float(
                "v2.oases",
                "sigma_max_world_units",
                s_max,
                U::WorldLength,
                Positive,
            );
        }

        if let Some(enabled) = v2.senescence.enabled {
            r.boolean("v2.senescence", "enabled", enabled);
        }
        r.float(
            "v2.senescence",
            "scale_seconds",
            v2.senescence.scale_seconds,
            U::SimulationSeconds,
            Positive,
        );
        r.float(
            "v2.senescence",
            "maximum_multiplier",
            v2.senescence.maximum_multiplier,
            U::Dimensionless,
            Positive,
        );

        let sf = &v2.emission.surplus_floor;
        if let Some(enabled) = sf.enabled {
            r.boolean("v2.emission.surplus_floor", "enabled", enabled);
        }
        if let Some(floor) = sf.energy_floor {
            r.float(
                "v2.emission.surplus_floor",
                "energy_floor",
                floor,
                U::Energy,
                NonNegative,
            );
        }

        let ex = &v2.emission.excitable;
        if let Some(enabled) = ex.enabled {
            r.boolean("v2.emission.excitable", "enabled", enabled);
        }
        r.text("v2.emission.excitable", "mode", &ex.mode);
        r.float(
            "v2.emission.excitable",
            "fire_readiness_threshold",
            ex.fire_readiness_threshold,
            U::Dimensionless,
            UnitPositive,
        );
        r.float(
            "v2.emission.excitable",
            "depletion_fraction",
            ex.depletion_fraction,
            U::Dimensionless,
            UnitClosed,
        );
        r.float(
            "v2.emission.excitable",
            "burst_rate_multiplier",
            ex.burst_rate_multiplier,
            U::Dimensionless,
            Positive,
        );
        r.float(
            "v2.emission.excitable",
            "recovery_time_seconds",
            ex.recovery_time_seconds,
            U::SimulationSeconds,
            Positive,
        );
        r.int(
            "v2.emission.excitable",
            "refractory_cooldown_ticks",
            ex.refractory_cooldown_ticks,
            U::Ticks,
            AtLeastOne,
        );

        let off = &v2.emission.offset;
        if let Some(enabled) = off.enabled {
            r.boolean("v2.emission.offset", "enabled", enabled);
        }
        r.text("v2.emission.offset", "mode", &off.mode);
        r.text(
            "v2.emission.offset",
            "pump_direction_mode",
            &off.pump_direction_mode,
        );
        r.float(
            "v2.emission.offset",
            "pump_sign",
            off.pump_sign,
            U::Dimensionless,
            FloatBound::Finite,
        );
        if let Some(distance) = off.distance_world_units {
            r.float(
                "v2.emission.offset",
                "distance_world_units",
                distance,
                U::WorldLength,
                NonNegative,
            );
        }

        if let Some(enabled) = v2.motor.enabled {
            r.boolean("v2.motor", "enabled", enabled);
        }
        r.float(
            "v2.motor",
            "energy_gradient_gain",
            v2.motor.energy_gradient_gain,
            U::Dimensionless,
            NonNegative,
        );
        r.float(
            "v2.motor",
            "chemical_gradient_gain",
            v2.motor.chemical_gradient_gain,
            U::Dimensionless,
            NonNegative,
        );
        r.float(
            "v2.motor",
            "chiral_contour_gain",
            v2.motor.chiral_contour_gain,
            U::Dimensionless,
            NonNegative,
        );
        r.float(
            "v2.motor",
            "heading_propulsion_fraction",
            v2.motor.heading_propulsion_fraction,
            U::Dimensionless,
            UnitClosed,
        );

        if let Some(enabled) = v2.budding.enabled {
            r.boolean("v2.budding", "enabled", enabled);
        }
        if let Some(radius) = v2.budding.local_offset_radius {
            r.float(
                "v2.budding",
                "local_offset_radius",
                radius,
                U::WorldLength,
                NonNegative,
            );
        }

        if let Some(enabled) = v2.crowding.enabled {
            r.boolean("v2.crowding", "enabled", enabled);
        }
        r.text("v2.crowding", "mode", &v2.crowding.mode);
        if let Some(radius) = v2.crowding.radius_world_units {
            r.float(
                "v2.crowding",
                "radius_world_units",
                radius,
                U::WorldLength,
                Positive,
            );
        }
        r.float(
            "v2.crowding",
            "arrest_threshold",
            v2.crowding.arrest_threshold,
            U::Count,
            Positive,
        );
        r.boolean("v2.crowding", "expose_sensor", v2.crowding.expose_sensor);

        if let Some(enabled) = v2.polarity.enabled {
            r.boolean("v2.polarity", "enabled", enabled);
        }
        r.float(
            "v2.polarity",
            "torque_mobility",
            v2.polarity.torque_mobility,
            U::PerSimulationSecond,
            NonNegative,
        );
        r.float(
            "v2.polarity",
            "gradient_cue_strength",
            v2.polarity.gradient_cue_strength,
            U::Dimensionless,
            NonNegative,
        );
        r.float(
            "v2.polarity",
            "initial_polarity_strength_max",
            v2.polarity.initial_polarity_strength_max,
            U::Dimensionless,
            UnitClosed,
        );
        r.float(
            "v2.polarity",
            "maximum_wedge_curvature",
            v2.polarity.maximum_wedge_curvature,
            U::Dimensionless,
            UnitClosed,
        );
        r.float(
            "v2.polarity",
            "polarity_mutation_std",
            v2.polarity.polarity_mutation_std,
            U::Dimensionless,
            NonNegative,
        );
        r.boolean(
            "v2.polarity",
            "oriented_lateral_mitosis",
            v2.polarity.oriented_lateral_mitosis,
        );
        if let Some(enabled) = v2.polarity.emt_gate_enabled {
            r.boolean("v2.polarity", "emt_gate_enabled", enabled);
        }

        if let Some(enabled) = v2.barrier.enabled {
            r.boolean("v2.barrier", "enabled", enabled);
        }
        r.text("v2.barrier", "mode", &v2.barrier.mode);
        r.float(
            "v2.barrier",
            "minimum_permeability",
            v2.barrier.minimum_permeability,
            U::Dimensionless,
            UnitPositive,
        );
        r.float(
            "v2.barrier",
            "occupancy_impedance",
            v2.barrier.occupancy_impedance,
            U::Dimensionless,
            NonNegative,
        );

        let ly = &v2.metabolism.lysis;
        if let Some(enabled) = ly.enabled {
            r.boolean("v2.metabolism.lysis", "enabled", enabled);
        }
        if let Some(floor) = ly.viability_energy_floor {
            r.float(
                "v2.metabolism.lysis",
                "viability_energy_floor",
                floor,
                U::Energy,
                NonNegative,
            );
        }
        r.text(
            "v2.metabolism.lysis",
            "corpse_direction",
            &ly.corpse_direction,
        );

        let wa = &v2.metabolism.waste;
        if let Some(enabled) = wa.enabled {
            r.boolean("v2.metabolism.waste", "enabled", enabled);
        }
        r.float(
            "v2.metabolism.waste",
            "byproduct_recycle_fraction",
            wa.byproduct_recycle_fraction,
            U::Dimensionless,
            UnitClosed,
        );

        let df = &v2.differentiation;
        if let Some(enabled) = df.enabled {
            r.boolean("v2.differentiation", "enabled", enabled);
        }
        r.int(
            "v2.differentiation",
            "state_width",
            df.state_width,
            U::Count,
            AtLeastOne,
        );
        r.float(
            "v2.differentiation",
            "epigenetic_gate_bias",
            df.epigenetic_gate_bias,
            U::Dimensionless,
            NonNegative,
        );
        r.text(
            "v2.differentiation",
            "state_inheritance",
            &df.state_inheritance,
        );
        r.float(
            "v2.differentiation",
            "inheritance_retention",
            df.inheritance_retention,
            U::Dimensionless,
            UnitClosed,
        );
        r.float(
            "v2.differentiation",
            "inheritance_jitter",
            df.inheritance_jitter,
            U::Dimensionless,
            NonNegative,
        );
        r.float(
            "v2.differentiation",
            "stem_asymmetry",
            df.stem_asymmetry,
            U::Dimensionless,
            FloatBound::Finite,
        );
        r.boolean(
            "v2.differentiation",
            "affinity_rotation_enabled",
            df.affinity_rotation_enabled,
        );
        r.boolean(
            "v2.differentiation",
            "emt_adhesion_enabled",
            df.emt_adhesion_enabled,
        );

        let nd = &df.notch_delta;
        if let Some(enabled) = nd.enabled {
            r.boolean("v2.differentiation.notch_delta", "enabled", enabled);
        }
        r.text("v2.differentiation.notch_delta", "mode", &nd.mode);
        r.float(
            "v2.differentiation.notch_delta",
            "inhibition_gain",
            nd.inhibition_gain,
            U::Dimensionless,
            UnitClosed,
        );

        let tr = &v2.trophallaxis;
        if let Some(enabled) = tr.enabled {
            r.boolean("v2.trophallaxis", "enabled", enabled);
        }
        r.float(
            "v2.trophallaxis",
            "conductance_per_second",
            tr.conductance_per_second,
            U::PerSimulationSecond,
            NonNegative,
        );
        r.float(
            "v2.trophallaxis",
            "transfer_efficiency",
            tr.transfer_efficiency,
            U::Dimensionless,
            UnitPositive,
        );
        if let Some(reserve) = tr.reserve_energy {
            r.float(
                "v2.trophallaxis",
                "reserve_energy",
                reserve,
                U::Energy,
                NonNegative,
            );
        }
        r.float(
            "v2.trophallaxis",
            "maximum_export_fraction",
            tr.maximum_export_fraction,
            U::Dimensionless,
            UnitPositive,
        );
        r.int(
            "v2.trophallaxis",
            "kin_cosine_exponent",
            tr.kin_cosine_exponent,
            U::Count,
            AtLeastOne,
        );
        r.float(
            "v2.trophallaxis",
            "kin_threshold",
            tr.kin_threshold,
            U::Dimensionless,
            UnitClosed,
        );

        let cad = &v2.contact.cadherin;
        if let Some(enabled) = cad.enabled {
            r.boolean("v2.contact.cadherin", "enabled", enabled);
        }
        r.float(
            "v2.contact.cadherin",
            "mutation_std",
            cad.mutation_std,
            U::Dimensionless,
            NonNegative,
        );

        let va = &v2.contact.velocity_alignment;
        if let Some(enabled) = va.enabled {
            r.boolean("v2.contact.velocity_alignment", "enabled", enabled);
        }
        r.float(
            "v2.contact.velocity_alignment",
            "strength",
            va.strength,
            U::Dimensionless,
            UnitClosed,
        );

        let vas = &v2.vascular;
        if let Some(enabled) = vas.enabled {
            r.boolean("v2.vascular", "enabled", enabled);
        }
        r.float(
            "v2.vascular",
            "minimum_conductance_ratio",
            vas.minimum_conductance_ratio,
            U::Dimensionless,
            UnitPositive,
        );
        if let Some(max_d) = vas.maximum_diffusion {
            r.float(
                "v2.vascular",
                "maximum_diffusion",
                max_d,
                U::Diffusion,
                Positive,
            );
        }
        r.float(
            "v2.vascular",
            "reinforcement_rate_per_second",
            vas.reinforcement_rate_per_second,
            U::PerSimulationSecond,
            Positive,
        );
        r.float(
            "v2.vascular",
            "pruning_rate_per_second",
            vas.pruning_rate_per_second,
            U::PerSimulationSecond,
            Positive,
        );
        r.float(
            "v2.vascular",
            "flux_half_saturation",
            vas.flux_half_saturation,
            U::EnergyPerSecond,
            Positive,
        );
        r.float(
            "v2.vascular",
            "flux_exponent",
            vas.flux_exponent,
            U::Dimensionless,
            Positive,
        );

        let car = &v2.carriers;
        if let Some(enabled) = car.enabled {
            r.boolean("v2.carriers", "enabled", enabled);
        }
        if let Some(d1) = car.trail_diffusion {
            r.float(
                "v2.carriers",
                "trail_diffusion",
                d1,
                U::Diffusion,
                NonNegative,
            );
        }
        if let Some(l1) = car.trail_decay_per_second {
            r.float(
                "v2.carriers",
                "trail_decay_per_second",
                l1,
                U::PerSimulationSecond,
                NonNegative,
            );
        }
        r.float(
            "v2.carriers",
            "trail_emission_share",
            car.trail_emission_share,
            U::Dimensionless,
            UnitPositive,
        );
        if let Some(scale) = car.trail_energy_density_scale {
            r.float(
                "v2.carriers",
                "trail_energy_density_scale",
                scale,
                U::EnergyDensity,
                Positive,
            );
        }

        if let Some(enabled) = v2.visual.enabled {
            r.boolean("v2.visual", "enabled", enabled);
        }
    }

    r.0
}
