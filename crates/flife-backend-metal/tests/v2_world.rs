//! Full-world `field-ecology-v2` Metal E2E bit-exact parity suite:
//!
//! 1. **All 13 closed pure-field V2 features enabled together** (`oases` +
//!    `senescence` + `surplus-emission` + `excitable-relay` + `wake-offset` +
//!    `so2-motor` + `so2-budding` + `hippo-crowding` (`cell-occupancy-v1`,
//!    `expose_sensor`) + `tight-junctions` + `lysis-recycling` +
//!    `symplectic-waste` + `vascular-field` + `dual-carrier-trails`; 34 planes,
//!    `sensor_width = 109`, `action_width = 42`) with ALL 5 Metal backends
//!    (`MetalField`, `MetalSensors`, `MetalUptake`, `MetalBackend`,
//!    `MetalSettle`) sharing ONE [`DeviceOwner`], tested in BOTH `f32` and
//!    `f16` over 64 ticks against pure CPU Reference:
//!    - bit-exact `World::digest()`, `FieldSummary`, births, deaths, newborn
//!      genomes, `Transfer::MetabolicByproduct`, and watched-organism
//!      transactions at every tick;
//!    - bit-exact `INV-03` ledger closure within `1e-6` per-tick relative
//!      residual and cumulative profile bound;
//!    - active GPU service on all 5 backends with 0 failures and 0 reference
//!      fallback ticks;
//!    - clean shared `DeviceOwner::usage()` (0 retiring, 0 leases, 0 running).
//! 2. **Recurrent `S = 2` (`field-ecology-v2` `110/46` with full `supercolony`
//!    including `V2-11`, and `106/44`) and FL-054 `S = 4` (`108/44`)** worlds
//!    comparing pure CPU Reference vs FrankenTorch CPU (`CpuBackend`) vs Metal
//!    (`MetalBackend` + all 4 other Metal backends on one shared `DeviceOwner`)
//!    in BOTH `f32` and `f16` over 64 ticks.
//! 3. **Explicit clean fallback (zero silent drift):** a V2 world using
//!    `fixed-world-gaussian-v1` (which `UptakeBackend` and `SettleBackend`
//!    explicitly refuse with `MissingCapability`) records both tick-1 refusals
//!    on `uptake_report()` and `settle_report()`, keeps `MetalField`,
//!    `MetalSensors`, and `MetalBackend` serving every tick on the shared
//!    owner, stays bit-identical to Reference at every tick, and leaves
//!    `DeviceOwner` idle.
//! 4. **All 15 closed V2 features with `[contact]` (`V2-08` `pcp-polarity` +
//!    `emt-gate` + `V2-11` `lysis-recycling` + `symplectic-waste`):** full
//!    contact-enabled V2 world (`34` planes, `109` sensors, odd `43` actions
//!    exercising F16 padded slot stride) over 64 ticks + starved lysis-firing
//!    sub-world over 16 ticks, asserting bit-exact `World::digest()`,
//!    `World::polarities()`, `DeathCause::Lysed`, `Transfer::MetabolicByproduct`,
//!    `Transfer::Recycling`, zero `Flow::TinyReservoir`, active GPU service on
//!    all 5 backends, and clean `DeviceOwner::usage()` in BOTH `f32` and `f16`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests assert directly on invariants and parity"
)]

use std::num::NonZeroUsize;

use flife_backend_cpu::CpuBackend;
use flife_backend_metal::{
    BackendBudget, DeviceOwner, Limits, MetalBackend, MetalField, MetalSensors, MetalSettle,
    MetalUptake,
};
use flife_core::{CoreError, Flow, Transfer, V2Feature, ValidatedConfig};
use flife_engine::{Arrangement, DeathCause, NumericalProfile, Stepped, World};
use flife_field::{DUAL_CARRIER_PLANES, FieldSummary, VASCULAR_INITIAL_CONDUCTANCE};
use flife_genome::Genome;

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

fn ecology_toml(model: &str, overrides: &[(&str, &str, &str)], tail: &str) -> String {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    let model_line = lines
        .iter()
        .position(|line| line.starts_with("model = "))
        .expect("model line");
    lines[model_line] = format!("model = \"{model}\"");
    for &(section, key, value) in overrides {
        let header = format!("[{section}]");
        let first = lines
            .iter()
            .position(|line| line.trim() == header)
            .expect("section");
        let existing = lines[first + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| first + 1 + offset);
        if let Some(line) = existing {
            lines[line] = format!("{key} = {value}");
        } else {
            lines.insert(first + 1, format!("{key} = {value}"));
        }
    }
    let mut out = lines.join("\n");
    if !tail.is_empty() {
        out.push('\n');
        out.push_str(tail);
    }
    out
}

fn open_owner() -> Option<DeviceOwner> {
    match DeviceOwner::open(Limits::UNBOUNDED) {
        Ok(owner) => Some(owner),
        Err(CoreError::MissingCapability { capability }) if !cfg!(target_os = "macos") => {
            assert_eq!(capability, "a Metal device");
            None
        }
        Err(error) => panic!("this host should have Metal: {error}"),
    }
}

/// Install ALL FIVE Metal backends (`MetalField`, `MetalSensors`, `MetalUptake`,
/// `MetalBackend`, `MetalSettle`) onto ONE shared [`DeviceOwner`].
fn install_all_metal_on_one_owner(world: &mut World) -> Option<DeviceOwner> {
    let owner = open_owner()?;
    world.set_field_backend(Some(Box::new(
        MetalField::with_owner(owner.clone()).expect("metal field"),
    )));
    world.set_sensor_backend(Some(Box::new(
        MetalSensors::with_owner(owner.clone()).expect("metal sensors"),
    )));
    world.set_uptake_backend(Some(Box::new(
        MetalUptake::with_owner(owner.clone()).expect("metal uptake"),
    )));
    world.set_controller_backend(Some(Box::new(
        MetalBackend::with_owner(owner.clone(), u64::MAX, BackendBudget::UNBOUNDED)
            .expect("metal controllers"),
    )));
    world.set_settle_backend(Some(Box::new(
        MetalSettle::with_owner(owner.clone()).expect("metal settle"),
    )));
    Some(owner)
}

fn same_genome(a: &Genome, b: &Genome) -> bool {
    let bits = |values: &[f64]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    let (x, y) = (a.traits(), b.traits());
    a.topology() == b.topology()
        && a.parameters().same_bits(&b.parameters())
        && bits(x.affinity()) == bits(y.affinity())
        && bits(x.motion_response()) == bits(y.motion_response())
        && x.sensing_scale().to_bits() == y.sensing_scale().to_bits()
}

fn summary_bits(summary: &FieldSummary) -> (u64, u64, Option<(u32, u32)>, u64) {
    (
        summary.energy_total.to_bits(),
        summary.max_cone_residual.to_bits(),
        summary.max_cone_residual_cell,
        summary.empty_cells,
    )
}

fn assert_owner_idle(owner: &DeviceOwner, label: &str) {
    let usage = owner.usage();
    assert_eq!(
        (
            usage.retiring,
            usage.read_leases,
            usage.write_leases,
            usage.running
        ),
        (0, [0; 3], 0, 0),
        "{label}: shared DeviceOwner left non-idle resources: {usage:?}"
    );
}

fn found_world(config: &ValidatedConfig) -> World {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let mut world = World::found(config, Arrangement::FreeList, profile).expect("found world");
    let workers = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    world
        .set_cpu_workers(NonZeroUsize::new(workers.min(8)).expect("nonzero"))
        .expect("workers");
    world
}

/// All 11 closed `field-ecology-v2` features active simultaneously on ONE
/// shared Metal `DeviceOwner` (`MetalField` + `MetalSensors` + `MetalUptake` +
/// `MetalBackend` + `MetalSettle`) in BOTH `f32` and `f16` over 64 ticks:
/// bit-exact `World::digest()`, ledger closure, births, newborn genomes,
/// watched-organism transactions, full GPU service across all 5 backends, and
/// zero leaked VRAM/leases.
#[test]
fn full_v2_all_closed_features_on_single_metal_owner_matches_reference_f32_and_f16() {
    if open_owner().is_none() {
        return;
    }
    let profile = NumericalProfile::reference_f32().expect("profile");

    for dtype in ["f32", "f16"] {
        let dtype_toml = format!("\"{dtype}\"");
        let overrides = [
            ("world", "width", "16.0"),
            ("world", "height", "16.0"),
            ("world", "grid_width", "32"),
            ("world", "grid_height", "32"),
            ("world", "tick_numerator", "1"),
            ("world", "tick_denominator", "32"),
            ("population", "initial_count", "96"),
            ("population", "maximum_count", "384"),
            ("population", "initial_energy", "3.5"),
            ("sources", "count", "4"),
            ("sources", "power_per_source", "220.0"),
            ("sources", "sigma_world_units", "2.0"),
            ("sources", "path_direction_interval_seconds", "2.0"),
            ("controller", "hidden_widths", "[64, 32]"),
            ("controller", "canonical_weight_dtype", dtype_toml.as_str()),
            ("reproduction", "cooldown_seconds", "0.05"),
            (
                "reproduction",
                "minimum_parent_energy_before_attempt",
                "1.5",
            ),
            ("architecture_mutation", "probability_per_birth", "0.25"),
            ("execution", "command_durability", "\"volatile-in-memory\""),
        ];
        // Supercolony enables all 12 Bundle B features (oases, senescence,
        // surplus-emission, excitable-relay, wake-offset, so2-motor,
        // so2-budding, tight-junctions, lysis-recycling, symplectic-waste,
        // vascular-field, dual-carrier-trails), plus hippo-crowding enabled in
        // pure-field cell-occupancy-v1 mode with expose_sensor = true.
        let tail = r#"
[v2]
profile = "supercolony"

[v2.crowding]
enabled = true
mode = "cell-occupancy-v1"
expose_sensor = true
"#;
        let toml = ecology_toml("field-ecology-v2", &overrides, tail);
        let config =
            ValidatedConfig::from_toml(&toml).expect("full closed pure-field V2 validates");

        // Confirm every closed pure-field V2 feature is active and ABI dimensions match.
        for feature in [
            V2Feature::Oases,
            V2Feature::Senescence,
            V2Feature::SurplusEmission,
            V2Feature::ExcitableRelay,
            V2Feature::WakeOffset,
            V2Feature::So2Motor,
            V2Feature::So2Budding,
            V2Feature::HippoCrowding,
            V2Feature::TightJunctions,
            V2Feature::LysisRecycling,
            V2Feature::SymplecticWaste,
            V2Feature::VascularField,
            V2Feature::DualCarrierTrails,
        ] {
            assert!(
                config.is_v2_feature_enabled(feature),
                "dtype={dtype}: expected {feature:?} enabled"
            );
        }
        let abi = config.v2_abi();
        assert_eq!(abi.field_planes, DUAL_CARRIER_PLANES);
        assert_eq!(abi.sensor_width, 109);
        assert_eq!(abi.action_width, 42);

        let mut reference = found_world(&config);
        let mut gpu = found_world(&config);
        let owner = install_all_metal_on_one_owner(&mut gpu).expect("metal owner");

        let watched = reference.population().ids().next();
        reference.watch(watched);
        gpu.watch(watched);

        let ticks = 64_u64;
        let mut max_rel = 0.0_f64;
        let mut saw_trail = false;
        let mut saw_vascular = false;
        let mut total_emissions = 0_usize;
        let mut total_births = 0_usize;
        let mut total_byproducts = 0.0_f64;
        for step in 1..=ticks {
            let Stepped::Published(expected) = reference.step().expect("ref step") else {
                panic!("dtype={dtype} tick={step}: reference paused");
            };
            let Stepped::Published(got) = gpu.step().expect("metal step") else {
                panic!("dtype={dtype} tick={step}: metal paused");
            };
            assert_eq!(
                reference.digest(),
                gpu.digest(),
                "dtype={dtype} tick={step}: World::digest diverged"
            );
            assert_eq!(
                summary_bits(&expected.field),
                summary_bits(&got.field),
                "dtype={dtype} tick={step}: FieldSummary diverged"
            );
            assert_eq!(
                (
                    expected.closure.closing().field().to_bits(),
                    expected.closure.closing().organisms().to_bits(),
                    expected.closure.residual().to_bits(),
                    expected
                        .closure
                        .credited(Transfer::MetabolicByproduct)
                        .to_bits(),
                ),
                (
                    got.closure.closing().field().to_bits(),
                    got.closure.closing().organisms().to_bits(),
                    got.closure.residual().to_bits(),
                    got.closure.credited(Transfer::MetabolicByproduct).to_bits(),
                ),
                "dtype={dtype} tick={step}: INV-03 closure or MetabolicByproduct diverged"
            );
            assert_eq!(
                expected.births, got.births,
                "dtype={dtype} tick={step}: births diverged"
            );
            assert_eq!(
                expected.deaths, got.deaths,
                "dtype={dtype} tick={step}: deaths diverged"
            );
            for birth in &got.births {
                let a = reference
                    .population()
                    .genome(birth.child)
                    .expect("ref child");
                let b = gpu.population().genome(birth.child).expect("gpu child");
                assert!(
                    same_genome(a, b),
                    "dtype={dtype} tick={step}: newborn genome {:?} diverged",
                    birth.child
                );
            }
            assert!(
                reference
                    .watched_transactions()
                    .eq(gpu.watched_transactions()),
                "dtype={dtype} tick={step}: watched transactions diverged"
            );

            let rel = got.closure.residual().abs() / got.closure.scale().max(1.0);
            max_rel = max_rel.max(rel);
            total_emissions += got.emitted;
            total_births += got.births.len();
            total_byproducts += got.closure.credited(Transfer::MetabolicByproduct);
            let trail_sum: f64 = gpu
                .field()
                .trail_energy()
                .expect("34-plane trail energy")
                .iter()
                .map(|&v| f64::from(v))
                .sum();
            saw_trail |= trail_sum > 0.0;
            let (gx, gy) = gpu.field().vascular_edges().expect("vascular edges");
            saw_vascular |= gx
                .iter()
                .chain(gy.iter())
                .any(|&g| (g - VASCULAR_INITIAL_CONDUCTANCE).abs() > 1e-4);
        }

        // INV-03 ledger closure bounds (§21.4).
        assert!(
            max_rel <= 1e-6,
            "dtype={dtype}: per-tick INV-03 relative residual {max_rel:e} > 1e-6"
        );
        let cum_norm = gpu.ledger().report().normalized.unwrap_or(0.0);
        assert!(
            cum_norm <= profile.residual.cumulative(),
            "dtype={dtype}: cumulative INV-03 residual {cum_norm:e} > {}",
            profile.residual.cumulative()
        );
        assert!(
            saw_vascular,
            "dtype={dtype}: expected vascular conductances to evolve"
        );
        assert!(
            total_emissions > 0 && saw_trail,
            "dtype={dtype}: expected dual-carrier emissions & positive Carrier-1 trail energy"
        );
        assert!(
            total_births > 0,
            "dtype={dtype}: expected live V2 births (so2-budding + hippo-crowding + metal birth mutation)"
        );
        assert!(
            total_byproducts > 0.0,
            "dtype={dtype}: expected positive V2-11 Transfer::MetabolicByproduct flux"
        );

        // Verify ALL FIVE Metal backends served every single tick with 0 failures!
        let f_rep = gpu.field_report();
        let s_rep = gpu.sensor_report();
        let u_rep = gpu.uptake_report();
        let c_rep = gpu.controller_report();
        let t_rep = gpu.settle_report();
        assert_eq!(
            f_rep.failure, None,
            "dtype={dtype}: field failed: {f_rep:?}"
        );
        assert_eq!(
            s_rep.failure, None,
            "dtype={dtype}: sensors failed: {s_rep:?}"
        );
        assert_eq!(
            u_rep.failure, None,
            "dtype={dtype}: uptake failed: {u_rep:?}"
        );
        assert_eq!(
            c_rep.failure, None,
            "dtype={dtype}: controllers failed: {c_rep:?}"
        );
        assert_eq!(
            t_rep.failure, None,
            "dtype={dtype}: settle failed: {t_rep:?}"
        );
        assert_eq!((f_rep.steps, f_rep.reference_steps), (ticks, 0));
        assert_eq!((f_rep.reductions, f_rep.reference_reductions), (ticks, 0));
        assert_eq!((f_rep.checks, f_rep.reference_checks), (ticks, 0));
        assert_eq!((s_rep.ticks, s_rep.reference_ticks), (ticks, 0));
        assert_eq!((u_rep.ticks, u_rep.reference_ticks), (ticks, 0));
        assert_eq!((t_rep.ticks, t_rep.reference_ticks), (ticks, 0));
        assert_eq!(c_rep.reference_ticks, 0);
        assert!(c_rep.batched > 0, "dtype={dtype}: controllers batched=0");

        // Shared DeviceOwner has active GPU command history and 0 leaked leases/running/retiring.
        assert!(owner.device_time().commands > 0);
        assert!(owner.usage().bytes > 0);
        assert_owner_idle(&owner, dtype);
    }
}

/// Recurrent `S = 2` (`field-ecology-v2`: `106/44` and `110/46`) and FL-054
/// `S = 4` (`108/44`) worlds on `MetalBackend` (with all 4 other Metal phase
/// backends on one shared `DeviceOwner`) AND FrankenTorch CPU (`CpuBackend`)
/// match pure CPU Reference bit-for-bit across 64 ticks in BOTH `f32` and `f16`.
#[test]
fn recurrent_s2_v2_world_on_metal_and_torch_matches_reference_f32_and_f16() {
    if open_owner().is_none() {
        return;
    }

    struct RecurrentCase<'a> {
        label: &'a str,
        model: &'a str,
        state_width: &'a str,
        tail: &'a str,
        expected_in: u32,
        expected_raw_out: u32,
    }

    let cases = [
        RecurrentCase {
            label: "v2 S=2 106/44 (so2-motor + recurrent S=2)",
            model: "field-ecology-v2",
            state_width: "2",
            tail: r#"
[v2]
profile = "minimal"

[v2.motor]
enabled = true
"#,
            expected_in: 106,
            expected_raw_out: 44,
        },
        RecurrentCase {
            label: "v2 S=2 110/46 (dual-carrier + vascular + motor + budding + S=2)",
            model: "field-ecology-v2",
            state_width: "2",
            tail: r#"
[v2]
profile = "supercolony"
"#,
            expected_in: 110,
            expected_raw_out: 46,
        },
        RecurrentCase {
            label: "v1 FL-054 S=4 108/44",
            model: "field-ecology-v1",
            state_width: "4",
            tail: "",
            expected_in: 108,
            expected_raw_out: 44,
        },
    ];

    for case in &cases {
        for dtype in ["f32", "f16"] {
            let dtype_toml = format!("\"{dtype}\"");
            let overrides = [
                ("world", "width", "16.0"),
                ("world", "height", "16.0"),
                ("world", "grid_width", "32"),
                ("world", "grid_height", "32"),
                ("world", "tick_numerator", "1"),
                ("world", "tick_denominator", "32"),
                ("population", "initial_count", "64"),
                ("population", "maximum_count", "256"),
                ("population", "initial_energy", "3.2"),
                ("sources", "count", "4"),
                ("sources", "power_per_source", "200.0"),
                ("sources", "sigma_world_units", "2.0"),
                ("sources", "path_direction_interval_seconds", "2.0"),
                ("controller", "family", "\"independent-recurrent-mlp-v1\""),
                ("controller", "recurrent_state_width", case.state_width),
                ("controller", "hidden_widths", "[32]"),
                ("controller", "canonical_weight_dtype", dtype_toml.as_str()),
                (
                    "architecture_mutation",
                    "catalog",
                    "\"recurrent-104-36-catalog-v1\"",
                ),
                ("reproduction", "cooldown_seconds", "0.05"),
                (
                    "reproduction",
                    "minimum_parent_energy_before_attempt",
                    "1.5",
                ),
                ("execution", "command_durability", "\"volatile-in-memory\""),
            ];
            let toml = ecology_toml(case.model, &overrides, case.tail);
            let config = ValidatedConfig::from_toml(&toml).expect("recurrent config validates");
            if case.model == "field-ecology-v2" {
                let abi = config.v2_abi();
                assert_eq!(abi.input_width, case.expected_in, "{}", case.label);
                assert_eq!(
                    abi.raw_output_width, case.expected_raw_out,
                    "{}",
                    case.label
                );
            }

            let mut reference = found_world(&config);
            let mut torch = found_world(&config);
            let mut gpu = found_world(&config);
            torch.set_controller_backend(Some(Box::new(CpuBackend::new(
                NonZeroUsize::new(4).expect("4"),
            ))));
            let owner = install_all_metal_on_one_owner(&mut gpu).expect("metal owner");

            let ticks = 64_u64;
            for step in 1..=ticks {
                assert!(matches!(
                    reference.step().expect("ref"),
                    Stepped::Published(_)
                ));
                assert!(matches!(
                    torch.step().expect("torch"),
                    Stepped::Published(_)
                ));
                assert!(matches!(gpu.step().expect("gpu"), Stepped::Published(_)));
                assert_eq!(
                    reference.digest(),
                    torch.digest(),
                    "{} dtype={dtype} tick={step}: CpuBackend diverged from Reference",
                    case.label
                );
                assert_eq!(
                    reference.digest(),
                    gpu.digest(),
                    "{} dtype={dtype} tick={step}: Metal diverged from Reference",
                    case.label
                );
            }

            let t_ctrl = torch.controller_report();
            let g_ctrl = gpu.controller_report();
            assert_eq!(t_ctrl.failure, None, "{} {dtype}: torch failed", case.label);
            assert_eq!(t_ctrl.reference_ticks, 0);
            assert!(t_ctrl.batched > 0);
            assert_eq!(g_ctrl.failure, None, "{} {dtype}: metal failed", case.label);
            assert_eq!(g_ctrl.reference_ticks, 0);
            assert!(g_ctrl.batched > 0);
            assert_eq!(gpu.field_report().failure, None);
            assert_eq!(gpu.sensor_report().failure, None);
            assert_eq!(gpu.uptake_report().failure, None);
            assert_eq!(gpu.settle_report().failure, None);
            assert_owner_idle(&owner, case.label);
        }
    }
}

/// Explicit clean fallback (zero silent drift): when a V2 world selects an
/// exchange kernel (`fixed-world-gaussian-v1`) that `UptakeBackend` and
/// `SettleBackend` explicitly refuse with `MissingCapability`, `World` records
/// the tick-1 refusals on `uptake_report()` and `settle_report()`, falls back
/// cleanly to CPU Reference for phases 5–8 while keeping `MetalField`,
/// `MetalSensors`, and `MetalBackend` serving every tick on GPU, preserves
/// bit-exact parity with pure CPU Reference, and leaves the shared
/// `DeviceOwner` idle.
#[test]
fn v2_explicit_clean_fallback_on_unsupported_exchange_kernel_preserves_bit_exact_parity() {
    if open_owner().is_none() {
        return;
    }
    let overrides = [
        ("world", "width", "16.0"),
        ("world", "height", "16.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("world", "exchange_kernel", "\"fixed-world-gaussian-v1\""),
        ("world", "exchange_radius_world_units", "1.0"),
        ("world", "tick_numerator", "1"),
        ("world", "tick_denominator", "32"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "192"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "180.0"),
        ("sources", "sigma_world_units", "2.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("controller", "hidden_widths", "[8]"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ];
    let tail = r#"
[v2]
profile = "supercolony"
"#;
    let toml = ecology_toml("field-ecology-v2", &overrides, tail);
    let config = ValidatedConfig::from_toml(&toml).expect("fixed-world-gaussian v2 validates");

    let mut reference = found_world(&config);
    let mut gpu = found_world(&config);
    let owner = install_all_metal_on_one_owner(&mut gpu).expect("metal owner");

    let ticks = 16_u64;
    for step in 1..=ticks {
        assert!(matches!(
            reference.step().expect("ref"),
            Stepped::Published(_)
        ));
        assert!(matches!(gpu.step().expect("gpu"), Stepped::Published(_)));
        assert_eq!(
            reference.digest(),
            gpu.digest(),
            "tick={step}: explicit fallback must preserve bit-exact digest parity"
        );
    }

    // Uptake and Settle backends explicitly refused on tick 1 with
    // MissingCapability and were dropped (`reference_ticks == 1`), while
    // MetalField, MetalSensors, and MetalBackend served every tick on GPU.
    let uptake_failure = gpu
        .uptake_report()
        .failure
        .as_deref()
        .expect("fixed-world-gaussian-v1 must record explicit uptake fallback");
    assert!(
        uptake_failure.contains("fixed-world-gaussian-v1"),
        "unexpected uptake failure detail: {uptake_failure}"
    );
    assert_eq!(gpu.uptake_report().ticks, 0);
    assert_eq!(gpu.uptake_report().reference_ticks, 1);

    let settle_failure = gpu
        .settle_report()
        .failure
        .as_deref()
        .expect("fixed-world-gaussian-v1 must record explicit settle fallback");
    assert!(
        settle_failure.contains("fixed-world-gaussian-v1"),
        "unexpected settle failure detail: {settle_failure}"
    );
    assert_eq!(gpu.settle_report().ticks, 0);
    assert_eq!(gpu.settle_report().reference_ticks, 1);

    assert_eq!(gpu.field_report().failure, None);
    assert_eq!(gpu.field_report().steps, ticks);
    assert_eq!(gpu.field_report().reductions, ticks);
    assert_eq!(gpu.field_report().checks, ticks);
    assert_eq!(gpu.sensor_report().failure, None);
    assert_eq!(gpu.sensor_report().ticks, ticks);
    assert_eq!(gpu.controller_report().failure, None);
    assert_eq!(gpu.controller_report().reference_ticks, 0);
    assert!(gpu.controller_report().batched > 0);
    assert_owner_idle(&owner, "fixed-world-gaussian fallback");
}

fn polarity_bits(world: &World) -> Vec<[u64; 2]> {
    world
        .polarities()
        .expect("pcp-polarity world exposes live polarities")
        .into_iter()
        .map(|[x, y]| [x.to_bits(), y.to_bits()])
        .collect()
}

/// Newly closed `V2-08` (`pcp-polarity` + `emt-gate`) and `V2-11`
/// (`lysis-recycling` + `symplectic-waste`) on ONE shared Metal `DeviceOwner`
/// (`MetalField` + `MetalSensors` + `MetalUptake` + `MetalBackend` +
/// `MetalSettle`) vs pure CPU Reference in BOTH `f32` and `f16`:
///
/// - **Part A (64 ticks):** `[contact]` + all 15 closed V2 features enabled
///   simultaneously (`34` planes, `109` sensors, odd `43` actions exercising
///   Metal F16 4-element slot stride alignment), asserting bit-exact
///   `World::digest()`, `World::polarities()`, `FieldSummary`, `INV-03`
///   closure, births, deaths, newborn genomes, watched transactions, active
///   polarity evolution, positive `Transfer::MetabolicByproduct`, `5/5` Metal
///   backend service, and clean `DeviceOwner::usage()`.
/// - **Part B (16 ticks):** starved `[contact]` + `pcp-polarity` + `emt-gate`
///   with `lysis-recycling` (`viability_energy_floor = 0.25`, `initial_energy =
///   0.20`) and `symplectic-waste`, guaranteeing `DeathCause::Lysed` fires on
///   Metal GPU with `Transfer::Recycling > 0`, `Flow::TinyReservoir == 0`,
///   bit-exact parity, active GPU service, and clean `DeviceOwner`.
#[test]
fn v2_contact_pcp_polarity_emt_gate_and_trophic_on_metal_matches_reference_f32_and_f16() {
    if open_owner().is_none() {
        return;
    }
    let profile = NumericalProfile::reference_f32().expect("profile");

    for dtype in ["f32", "f16"] {
        let dtype_toml = format!("\"{dtype}\"");

        // Part A: all 15 closed V2 features simultaneously with `[contact]`.
        let overrides = [
            ("world", "width", "16.0"),
            ("world", "height", "16.0"),
            ("world", "grid_width", "32"),
            ("world", "grid_height", "32"),
            ("world", "tick_numerator", "1"),
            ("world", "tick_denominator", "32"),
            ("population", "initial_count", "96"),
            ("population", "maximum_count", "384"),
            ("population", "initial_energy", "3.5"),
            ("sources", "count", "4"),
            ("sources", "power_per_source", "220.0"),
            ("sources", "sigma_world_units", "2.0"),
            ("sources", "path_direction_interval_seconds", "2.0"),
            ("controller", "hidden_widths", "[64, 32]"),
            ("controller", "canonical_weight_dtype", dtype_toml.as_str()),
            ("reproduction", "cooldown_seconds", "0.05"),
            (
                "reproduction",
                "minimum_parent_energy_before_attempt",
                "1.5",
            ),
            ("architecture_mutation", "probability_per_birth", "0.25"),
            ("execution", "command_durability", "\"volatile-in-memory\""),
        ];
        let tail = r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 2.0
sweeps = 3

[v2]
profile = "supercolony"

[v2.crowding]
enabled = true
mode = "local-neighbors-v1"
arrest_threshold = 6.0
expose_sensor = true

[v2.polarity]
enabled = true
torque_mobility = 2.0
gradient_cue_strength = 0.25
initial_polarity_strength_max = 0.8
maximum_wedge_curvature = 0.25
polarity_mutation_std = 0.05
oriented_lateral_mitosis = true
emt_gate_enabled = true
"#;
        let toml = ecology_toml("field-ecology-v2", &overrides, tail);
        let config =
            ValidatedConfig::from_toml(&toml).expect("15-feature contact V2 config validates");

        for feature in [
            V2Feature::Oases,
            V2Feature::Senescence,
            V2Feature::SurplusEmission,
            V2Feature::ExcitableRelay,
            V2Feature::WakeOffset,
            V2Feature::So2Motor,
            V2Feature::So2Budding,
            V2Feature::HippoCrowding,
            V2Feature::PcpPolarity,
            V2Feature::EmtGate,
            V2Feature::TightJunctions,
            V2Feature::LysisRecycling,
            V2Feature::SymplecticWaste,
            V2Feature::VascularField,
            V2Feature::DualCarrierTrails,
        ] {
            assert!(
                config.is_v2_feature_enabled(feature),
                "Part A dtype={dtype}: expected {feature:?} enabled"
            );
        }
        let abi = config.v2_abi();
        assert_eq!(abi.field_planes, DUAL_CARRIER_PLANES);
        assert_eq!(abi.sensor_width, 109);
        assert_eq!(
            abi.action_width, 43,
            "so2-motor(+4) + so2-budding(+2) + emt-gate(+1) = 43"
        );

        let mut reference = found_world(&config);
        let mut gpu = found_world(&config);
        assert!(reference.tracks_polarity() && gpu.tracks_polarity());
        let owner = install_all_metal_on_one_owner(&mut gpu).expect("metal owner");

        let watched = reference.population().ids().next();
        reference.watch(watched);
        gpu.watch(watched);

        let initial_pols = polarity_bits(&gpu);
        assert_eq!(initial_pols, polarity_bits(&reference));
        let mut polarities_evolved = false;
        let mut max_rel = 0.0_f64;
        let mut saw_trail = false;
        let mut saw_vascular = false;
        let mut total_emissions = 0_usize;
        let mut total_births = 0_usize;
        let mut total_byproducts = 0.0_f64;

        let ticks = 64_u64;
        for step in 1..=ticks {
            let Stepped::Published(expected) = reference.step().expect("ref step") else {
                panic!("Part A dtype={dtype} tick={step}: reference paused");
            };
            let Stepped::Published(got) = gpu.step().expect("metal step") else {
                panic!("Part A dtype={dtype} tick={step}: metal paused");
            };
            assert_eq!(
                reference.digest(),
                gpu.digest(),
                "Part A dtype={dtype} tick={step}: World::digest diverged"
            );
            let ref_pols = polarity_bits(&reference);
            let gpu_pols = polarity_bits(&gpu);
            assert_eq!(
                ref_pols, gpu_pols,
                "Part A dtype={dtype} tick={step}: World::polarities diverged"
            );
            polarities_evolved |= gpu_pols != initial_pols;

            assert_eq!(
                summary_bits(&expected.field),
                summary_bits(&got.field),
                "Part A dtype={dtype} tick={step}: FieldSummary diverged"
            );
            assert_eq!(
                (
                    expected.closure.closing().field().to_bits(),
                    expected.closure.closing().organisms().to_bits(),
                    expected.closure.residual().to_bits(),
                    expected
                        .closure
                        .credited(Transfer::MetabolicByproduct)
                        .to_bits(),
                ),
                (
                    got.closure.closing().field().to_bits(),
                    got.closure.closing().organisms().to_bits(),
                    got.closure.residual().to_bits(),
                    got.closure.credited(Transfer::MetabolicByproduct).to_bits(),
                ),
                "Part A dtype={dtype} tick={step}: INV-03 closure or MetabolicByproduct diverged"
            );
            assert_eq!(expected.births, got.births);
            assert_eq!(expected.deaths, got.deaths);
            for birth in &got.births {
                let a = reference
                    .population()
                    .genome(birth.child)
                    .expect("ref child");
                let b = gpu.population().genome(birth.child).expect("gpu child");
                assert!(
                    same_genome(a, b),
                    "Part A dtype={dtype} tick={step}: newborn genome {:?} diverged",
                    birth.child
                );
            }
            assert!(
                reference
                    .watched_transactions()
                    .eq(gpu.watched_transactions()),
                "Part A dtype={dtype} tick={step}: watched transactions diverged"
            );

            let rel = got.closure.residual().abs() / got.closure.scale().max(1.0);
            max_rel = max_rel.max(rel);
            total_emissions += got.emitted;
            total_births += got.births.len();
            total_byproducts += got.closure.credited(Transfer::MetabolicByproduct);
            let trail_sum: f64 = gpu
                .field()
                .trail_energy()
                .expect("34-plane trail energy")
                .iter()
                .map(|&v| f64::from(v))
                .sum();
            saw_trail |= trail_sum > 0.0;
            let (gx, gy) = gpu.field().vascular_edges().expect("vascular edges");
            saw_vascular |= gx
                .iter()
                .chain(gy.iter())
                .any(|&g| (g - VASCULAR_INITIAL_CONDUCTANCE).abs() > 1e-4);
        }

        assert!(
            max_rel <= 1e-6,
            "Part A dtype={dtype}: per-tick INV-03 residual {max_rel:e} > 1e-6"
        );
        let cum_norm = gpu.ledger().report().normalized.unwrap_or(0.0);
        assert!(
            cum_norm <= profile.residual.cumulative(),
            "Part A dtype={dtype}: cumulative INV-03 residual {cum_norm:e} > {}",
            profile.residual.cumulative()
        );
        assert!(
            polarities_evolved,
            "Part A dtype={dtype}: expected V2-08 PCP polarities to evolve"
        );
        assert!(
            total_byproducts > 0.0,
            "Part A dtype={dtype}: expected positive V2-11 Transfer::MetabolicByproduct flux"
        );
        assert!(
            saw_vascular,
            "Part A dtype={dtype}: expected vascular conductances to evolve"
        );
        assert!(
            total_emissions > 0 && saw_trail,
            "Part A dtype={dtype}: expected dual-carrier emissions & trail energy"
        );
        assert!(
            total_births > 0,
            "Part A dtype={dtype}: expected live V2 births under pcp-polarity + hippo-crowding"
        );

        let f_rep = gpu.field_report();
        let s_rep = gpu.sensor_report();
        let u_rep = gpu.uptake_report();
        let c_rep = gpu.controller_report();
        let t_rep = gpu.settle_report();
        assert_eq!(f_rep.failure, None, "Part A {dtype} field: {f_rep:?}");
        assert_eq!(s_rep.failure, None, "Part A {dtype} sensors: {s_rep:?}");
        assert_eq!(u_rep.failure, None, "Part A {dtype} uptake: {u_rep:?}");
        assert_eq!(c_rep.failure, None, "Part A {dtype} ctrl: {c_rep:?}");
        assert_eq!(t_rep.failure, None, "Part A {dtype} settle: {t_rep:?}");
        assert_eq!((f_rep.steps, f_rep.reference_steps), (ticks, 0));
        assert_eq!((f_rep.reductions, f_rep.reference_reductions), (ticks, 0));
        assert_eq!((f_rep.checks, f_rep.reference_checks), (ticks, 0));
        assert_eq!((s_rep.ticks, s_rep.reference_ticks), (ticks, 0));
        assert_eq!((u_rep.ticks, u_rep.reference_ticks), (ticks, 0));
        assert_eq!((t_rep.ticks, t_rep.reference_ticks), (ticks, 0));
        assert_eq!(c_rep.reference_ticks, 0);
        assert!(c_rep.batched > 0);
        assert_owner_idle(&owner, &format!("Part A contact+pcp+trophic {dtype}"));

        // Part B: starved V2-11 lysis-recycling firing on Metal GPU alongside V2-08.
        let starved_overrides = [
            ("world", "width", "16.0"),
            ("world", "height", "16.0"),
            ("world", "grid_width", "32"),
            ("world", "grid_height", "32"),
            ("world", "tick_numerator", "1"),
            ("world", "tick_denominator", "64"),
            ("population", "initial_count", "16"),
            ("population", "maximum_count", "64"),
            ("population", "initial_energy", "0.20"),
            ("sources", "count", "1"),
            ("sources", "power_per_source", "1e-6"),
            ("death", "residual_energy_recycle_fraction", "0.80"),
            ("controller", "hidden_widths", "[32]"),
            ("controller", "canonical_weight_dtype", dtype_toml.as_str()),
            (
                "reproduction",
                "minimum_parent_energy_before_attempt",
                "1.5",
            ),
            ("execution", "command_durability", "\"volatile-in-memory\""),
        ];
        let starved_tail = r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 2.0
sweeps = 3

[v2]
profile = "minimal"

[v2.emission.surplus_floor]
enabled = true
energy_floor = 0.50

[v2.polarity]
enabled = true
emt_gate_enabled = true

[v2.metabolism.lysis]
enabled = true
viability_energy_floor = 0.25
corpse_direction = "symplectic-affinity"

[v2.metabolism.waste]
enabled = true
"#;
        let starved_toml = ecology_toml("field-ecology-v2", &starved_overrides, starved_tail);
        let starved_cfg =
            ValidatedConfig::from_toml(&starved_toml).expect("starved lysis+pcp config validates");
        let mut s_ref = found_world(&starved_cfg);
        let mut s_gpu = found_world(&starved_cfg);
        let s_owner = install_all_metal_on_one_owner(&mut s_gpu).expect("metal owner");

        let mut lysed_deaths = 0_usize;
        let mut recycling_credited = 0.0_f64;
        let starved_ticks = 16_u64;
        for step in 1..=starved_ticks {
            let Stepped::Published(expected) = s_ref.step().expect("starved ref") else {
                panic!("Part B dtype={dtype} tick={step}: ref paused");
            };
            let Stepped::Published(got) = s_gpu.step().expect("starved gpu") else {
                panic!("Part B dtype={dtype} tick={step}: gpu paused");
            };
            assert_eq!(
                s_ref.digest(),
                s_gpu.digest(),
                "Part B dtype={dtype} tick={step}: starved lysis World::digest diverged"
            );
            assert_eq!(polarity_bits(&s_ref), polarity_bits(&s_gpu));
            assert_eq!(expected.deaths, got.deaths);
            assert_eq!(
                got.closure.flow(Flow::TinyReservoir),
                0.0,
                "Part B dtype={dtype} tick={step}: lysis must not book Flow::TinyReservoir"
            );
            lysed_deaths += got
                .deaths
                .iter()
                .filter(|d| matches!(d.cause, DeathCause::Lysed(_)))
                .count();
            recycling_credited += got.closure.credited(Transfer::Recycling);
            let rel = got.closure.residual().abs() / got.closure.scale().max(1.0);
            assert!(
                rel <= 1e-6,
                "Part B dtype={dtype} tick={step}: INV-03 residual {rel:e} > 1e-6"
            );
        }
        assert_eq!(
            lysed_deaths, 16,
            "Part B dtype={dtype}: expected all 16 starved organisms to lyse via DeathCause::Lysed"
        );
        assert!(
            recycling_credited > 0.0,
            "Part B dtype={dtype}: expected positive Transfer::Recycling from corpse lysis"
        );
        assert_eq!(s_gpu.field_report().failure, None);
        assert_eq!(s_gpu.field_report().steps, starved_ticks);
        assert_eq!(s_gpu.sensor_report().failure, None);
        assert_eq!(s_gpu.uptake_report().failure, None);
        assert_eq!(s_gpu.controller_report().failure, None);
        assert_eq!(s_gpu.controller_report().reference_ticks, 0);
        assert!(s_gpu.controller_report().batched >= 16);
        assert_eq!(s_gpu.settle_report().failure, None);
        assert_eq!(s_gpu.settle_report().reference_ticks, 0);
        assert_owner_idle(&s_owner, &format!("Part B starved lysis+pcp {dtype}"));
    }
}
