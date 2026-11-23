//! V2-04 tests: Surplus-gated emission floor (`surplus-emission`), excitable
//! refractory pulse relay (`excitable-relay`), and wake/pump deposit offset
//! (`wake-offset`).
//!
//! Covers:
//! - Exact known-answer unit tests for surplus floor (`E < floor`, `E == floor`,
//!   `E > floor`), vesicle readiness fire/deplete/recover (`expm1_neg` positive
//!   sign, zero hidden energy), `tick-cooldown-v1`, `surplus == 0` / invalid
//!   direction recovery guard (NEVER depletes `r_i`), and `wake-offset`
//!   (`v = [0, 0]`, all 4 torus seams, `polarized-pump-v1` fallback without
//!   `V2-08`, and `neural-output-v1`).
//! - Bit-exact baseline parity when all 3 toggles are disabled (`0` extra
//!   population column bytes, identical `World::digest()`).
//! - Checkpoint v3 round-trip, `C.7`, `C.8`, `World::footprint()` (`INV-15`),
//!   and `Intervention::IntroduceOrganisms` (`r_i = 0.2`, `INV-07`).
//! - E2E high-`r_max` (`4×` baseline) Stage-W survival & pulsatile emission
//!   verification with `INV-03 < 1e-5`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration tests assert directly on invariants and fixtures"
)]

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::clock::{Tick, TimeStep};
use flife_core::ids::OrganismId;
use flife_core::ledger::{EnergyBudget, sub_down};
use flife_core::numerics::{expm1_neg, sigmoid, tanh};
use flife_core::{FeatureOverride, ValidatedConfig};
use flife_engine::{
    Advance, Arrangement, EmissionContext, EmissionOffsetRule, EmissionPolicy, EmissionV2Rules,
    ExcitableMode, INITIAL_SECRETORY_READINESS, Intervention, NumericalProfile, OffsetMode,
    PumpDirection, Stepped, SurplusFloorRule, WORLD_CHECKPOINT_CHUNKS, World,
};
use flife_field::state::CHANNELS;
use flife_field::{BufferLimits, Deposits, ExchangeKernel, FieldGrid};
use flife_neural::Emission;

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
        let line = lines[first + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| first + 1 + offset)
            .expect("key");
        lines[line] = format!("{key} = {value}");
    }
    let mut out = lines.join("\n");
    if !tail.is_empty() {
        out.push('\n');
        out.push_str(tail);
    }
    out
}

fn small_overrides() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        ("world", "width", "16.0"),
        ("world", "height", "16.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("world", "tick_numerator", "1"),
        ("world", "tick_denominator", "32"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "256"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "160.0"),
        ("sources", "sigma_world_units", "3.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.5"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.0",
        ),
        ("reproduction", "maximum_attempt_rate_per_second", "4.0"),
        ("reproduction", "cooldown_seconds", "0.25"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ]
}

fn unit_x_emission(spend_fraction: f64) -> Emission {
    let mut direction = [0.0; CHANNELS];
    direction[0] = 1.0;
    Emission {
        direction,
        spend_fraction,
    }
}

#[test]
fn surplus_floor_exact_zero_below_floor_and_ledger() {
    let grid = FieldGrid::new(16, 16, 16.0, 16.0).expect("grid");
    let policy = EmissionPolicy::new(0.8).expect("policy");
    let floor = 1.0_f64;
    let emission = unit_x_emission(0.25);

    // 1. E < floor -> exact zero spend and zero deposit.
    let mut budget_low = EnergyBudget::new(0.6).expect("low");
    let mut deposits = Deposits::with_capacity(&grid, 8, &BufferLimits::host()).expect("dep");
    let s_low = policy
        .emit_with_floor(
            &grid,
            ExchangeKernel::GridCicV1,
            OrganismId::new(1),
            [4.0, 4.0],
            Some(&emission),
            floor,
            &mut budget_low,
            &mut deposits,
        )
        .expect("emit low");
    assert_eq!(s_low.cost.requested, 0.0);
    assert_eq!(s_low.cost.paid, 0.0);
    assert_eq!(s_low.offered, 0.0);
    assert_eq!(budget_low.remaining(), 0.6);
    assert!(deposits.is_empty());

    // 2. E == floor -> exact zero spend and zero deposit.
    let mut budget_eq = EnergyBudget::new(floor).expect("eq");
    let s_eq = policy
        .emit_with_floor(
            &grid,
            ExchangeKernel::GridCicV1,
            OrganismId::new(2),
            [4.0, 4.0],
            Some(&emission),
            floor,
            &mut budget_eq,
            &mut deposits,
        )
        .expect("emit eq");
    assert_eq!(s_eq.cost.paid, 0.0);
    assert_eq!(s_eq.offered, 0.0);
    assert_eq!(budget_eq.remaining(), floor);
    assert!(deposits.is_empty());

    // 3. E > floor -> exact f * (E - floor) spend and ρ * w offered.
    let mut budget_hi = EnergyBudget::new(3.0).expect("hi");
    let s_hi = policy
        .emit_with_floor(
            &grid,
            ExchangeKernel::GridCicV1,
            OrganismId::new(3),
            [4.0, 4.0],
            Some(&emission),
            floor,
            &mut budget_hi,
            &mut deposits,
        )
        .expect("emit hi");
    let want_w = 0.25 * (3.0 - floor); // 0.5
    let want_offered = 0.8 * want_w; // 0.4
    assert_eq!(s_hi.cost.requested, want_w);
    assert_eq!(s_hi.cost.paid, want_w);
    assert_eq!(s_hi.offered, want_offered);
    assert_eq!(budget_hi.remaining(), 3.0 - want_w);
    assert!(!deposits.is_empty());
}

#[test]
fn vesicle_readiness_fire_deplete_recover_exact() {
    let grid = FieldGrid::new(16, 16, 16.0, 16.0).expect("grid");
    let step = TimeStep::new(1, 32).expect("step");
    let dt = step.seconds_per_tick();
    let policy = EmissionPolicy::new(0.8).expect("policy");
    let r_max = 1.0_f64;
    let burst_mult = 5.0_f64;
    let tau_rec = 1.0_f64;
    let alpha_dep = 0.85_f64;
    let r_fire = 0.5_f64;
    let rules = EmissionV2Rules {
        surplus_floor: Some(SurplusFloorRule { energy_floor: 0.5 }),
        excitable: Some(ExcitableMode::VesicleReadiness {
            fire_readiness_threshold: r_fire,
            depletion_fraction: alpha_dep,
            burst_rate_multiplier: burst_mult,
            recovery_time_seconds: tau_rec,
        }),
        offset: None,
        max_spend_rate: r_max,
    };
    let emission = unit_x_emission(0.05);
    let raw_spend_logit = 0.0_f64; // sigma(0) = 0.5
    let alpha_rec = expm1_neg(dt / tau_rec).expect("positive recovery alpha");
    assert!(
        alpha_rec > 0.0 && alpha_rec < 0.1,
        "expm1_neg(dt/tau) must be positive: {alpha_rec}"
    );

    let mut r = INITIAL_SECRETORY_READINESS;
    let mut fired_ticks = Vec::new();
    let mut deposits = Deposits::with_capacity(&grid, 64, &BufferLimits::host()).expect("dep");

    for t in 0_u64..64 {
        let mut budget = EnergyBudget::new(2.5).expect("budget");
        let ctx = EmissionContext {
            position: [4.0, 4.0],
            velocity: [0.0, 0.0],
            energy_gradient: [0.0, 0.0],
            polarity: None,
            raw_spend_logit,
            raw_pump_logit: None,
            tick: Tick::new(t),
            secretory_readiness: r,
            emission_cooldown_deadline: Tick::new(0),
        };
        let (settled, plan) = policy
            .emit_v2(
                &grid,
                ExchangeKernel::GridCicV1,
                step,
                &rules,
                OrganismId::new(1),
                Some(&emission),
                &ctx,
                &mut budget,
                &mut deposits,
            )
            .expect("emit_v2");
        let r_f64 = f64::from(r);
        if r_f64 >= r_fire {
            fired_ticks.push(t);
            let want_burst =
                expm1_neg(r_max * burst_mult * sigmoid(raw_spend_logit).expect("sig") * dt)
                    .expect("burst");
            let want_f_eff = r_f64 * want_burst;
            let want_w = want_f_eff * (2.5 - 0.5);
            assert_eq!(settled.cost.paid.to_bits(), want_w.to_bits());
            assert_eq!(budget.remaining(), sub_down(2.5, settled.cost.paid));
            let want_r_next = ((1.0 - alpha_dep) * r_f64) as f32;
            assert_eq!(plan.next_readiness.to_bits(), want_r_next.to_bits());
        } else {
            assert_eq!(settled.cost.paid, 0.0);
            assert_eq!(settled.offered, 0.0);
            assert_eq!(budget.remaining(), 2.5);
            let want_r_next = (r_f64 + (1.0 - r_f64) * alpha_rec).min(1.0) as f32;
            assert_eq!(plan.next_readiness.to_bits(), want_r_next.to_bits());
        }
        r = plan.next_readiness;
    }
    // Starting at r=0.2 with tau=1s (32 ticks), 1 - r decays as 0.8 * exp(-t/32)
    // reaching r >= 0.5 at tick 16, depleting to ~0.077, then recovering over 20
    // ticks + 1 fire tick -> exact 21-tick refractory period: ticks [16, 37, 58].
    assert_eq!(
        fired_ticks,
        [16, 37, 58],
        "expected exact 21-tick refractory pulses [16, 37, 58] over 64 ticks"
    );
}

#[test]
fn excitable_surplus_zero_recovers_instead_of_depleting() {
    let grid = FieldGrid::new(16, 16, 16.0, 16.0).expect("grid");
    let step = TimeStep::new(1, 32).expect("step");
    let policy = EmissionPolicy::new(0.8).expect("policy");
    let rules = EmissionV2Rules {
        surplus_floor: Some(SurplusFloorRule { energy_floor: 1.0 }),
        excitable: Some(ExcitableMode::VesicleReadiness {
            fire_readiness_threshold: 0.5,
            depletion_fraction: 0.85,
            burst_rate_multiplier: 5.0,
            recovery_time_seconds: 1.0,
        }),
        offset: None,
        max_spend_rate: 1.0,
    };
    let emission = unit_x_emission(0.1);
    let mut deposits = Deposits::with_capacity(&grid, 8, &BufferLimits::host()).expect("dep");
    let base_ctx = EmissionContext {
        position: [4.0, 4.0],
        velocity: [0.0, 0.0],
        energy_gradient: [0.0, 0.0],
        polarity: None,
        raw_spend_logit: 0.0,
        raw_pump_logit: None,
        tick: Tick::new(10),
        secretory_readiness: 0.8,
        emission_cooldown_deadline: Tick::new(0),
    };

    // Case 1: E <= floor (`surplus == 0`) with high readiness `r = 0.8` -> MUST recover, NOT deplete!
    let mut budget_floor = EnergyBudget::new(1.0).expect("floor");
    let (s1, p1) = policy
        .emit_v2(
            &grid,
            ExchangeKernel::GridCicV1,
            step,
            &rules,
            OrganismId::new(1),
            Some(&emission),
            &base_ctx,
            &mut budget_floor,
            &mut deposits,
        )
        .expect("emit surplus 0");
    assert_eq!(s1.cost.paid, 0.0);
    assert!(
        p1.next_readiness > 0.8,
        "surplus==0 must RECOVER readiness (> 0.8), got {}",
        p1.next_readiness
    );

    // Case 2: emission == None (closed gate or zero direction) with high surplus -> MUST recover!
    let mut budget_rich = EnergyBudget::new(3.0).expect("rich");
    let (s2, p2) = policy
        .emit_v2(
            &grid,
            ExchangeKernel::GridCicV1,
            step,
            &rules,
            OrganismId::new(2),
            None,
            &base_ctx,
            &mut budget_rich,
            &mut deposits,
        )
        .expect("emit none");
    assert_eq!(s2.cost.paid, 0.0);
    assert_eq!(
        p2.next_readiness.to_bits(),
        p1.next_readiness.to_bits(),
        "emission==None must take the exact same recovery branch as surplus==0"
    );
    assert!(deposits.is_empty());
}

#[test]
fn wake_offset_zero_velocity_and_torus_wrap() {
    let grid = FieldGrid::new(32, 32, 16.0, 16.0).expect("grid");
    let step = TimeStep::new(1, 32).expect("step");
    let emission = unit_x_emission(0.1);
    let delta = 0.25_f64;

    // 1. wake-velocity-v1 at v = [0, 0] -> exact x_post, no NaN!
    let wake_rules = EmissionV2Rules {
        surplus_floor: None,
        excitable: None,
        offset: Some(EmissionOffsetRule {
            mode: OffsetMode::WakeVelocity,
            distance_world_units: delta,
        }),
        max_spend_rate: 1.0,
    };
    let still_ctx = EmissionContext {
        position: [0.05, 0.05],
        velocity: [0.0, 0.0],
        energy_gradient: [0.0, 0.0],
        polarity: None,
        raw_spend_logit: 0.0,
        raw_pump_logit: None,
        tick: Tick::new(0),
        secretory_readiness: 0.2,
        emission_cooldown_deadline: Tick::new(0),
    };
    let plan_still = wake_rules
        .plan(&grid, step, 2.0, false, Some(&emission), &still_ctx)
        .expect("still");
    assert_eq!(plan_still.deposit_position, [0.05, 0.05]);

    // 2. Wrap across all 4 torus seams (x=0, x=L, y=0, y=L).
    for (pos, vel, want) in [
        ([0.05, 8.0], [2.0, 0.0], [15.8, 8.0]),
        ([15.95, 8.0], [-2.0, 0.0], [0.2, 8.0]),
        ([8.0, 0.05], [0.0, 2.0], [8.0, 15.8]),
        ([8.0, 15.95], [0.0, -2.0], [8.0, 0.2]),
    ] {
        let ctx = EmissionContext {
            position: pos,
            velocity: vel,
            ..still_ctx
        };
        let plan = wake_rules
            .plan(&grid, step, 2.0, false, Some(&emission), &ctx)
            .expect("seam wrap");
        assert!(
            (plan.deposit_position[0] - want[0]).abs() < 1e-12
                && (plan.deposit_position[1] - want[1]).abs() < 1e-12,
            "pos={pos:?}, vel={vel:?}: got {:?}, want {want:?}",
            plan.deposit_position
        );
    }

    // 3. polarized-pump-v1 fallback without V2-08 (both fixed-sign-v1 and neural-output-v1).
    let l_grad = 2.0_f64;
    let pump_fixed = EmissionV2Rules {
        surplus_floor: None,
        excitable: None,
        offset: Some(EmissionOffsetRule {
            mode: OffsetMode::PolarizedPump {
                direction: PumpDirection::FixedSign(-1.0),
                gradient_length_scale: l_grad,
            },
            distance_world_units: delta,
        }),
        max_spend_rate: 1.0,
    };
    let grad = [0.5, 0.0];
    let g0 = 0.05 / l_grad;
    let expected_axis_x = grad[0] / (grad[0] * grad[0] + g0 * g0).sqrt();
    let ctx_grad = EmissionContext {
        position: [4.0, 4.0],
        energy_gradient: grad,
        ..still_ctx
    };
    let p_fixed = pump_fixed
        .plan(&grid, step, 2.0, false, Some(&emission), &ctx_grad)
        .expect("pump fixed");
    assert!(
        (p_fixed.deposit_position[0] - (4.0 - delta * expected_axis_x)).abs() < 1e-12
            && (p_fixed.deposit_position[1] - 4.0).abs() < 1e-12
    );

    // Zero gradient must never NaN.
    let ctx_zero_grad = EmissionContext {
        position: [4.0, 4.0],
        energy_gradient: [0.0, 0.0],
        ..still_ctx
    };
    let p_zero = pump_fixed
        .plan(&grid, step, 2.0, false, Some(&emission), &ctx_zero_grad)
        .expect("zero grad");
    assert_eq!(p_zero.deposit_position, [4.0, 4.0]);

    // neural-output-v1 scales by tanh(o_pump).
    let pump_neural = EmissionV2Rules {
        surplus_floor: None,
        excitable: None,
        offset: Some(EmissionOffsetRule {
            mode: OffsetMode::PolarizedPump {
                direction: PumpDirection::NeuralOutput,
                gradient_length_scale: l_grad,
            },
            distance_world_units: delta,
        }),
        max_spend_rate: 1.0,
    };
    let o_pump = 0.75_f64;
    let ctx_neural = EmissionContext {
        position: [4.0, 4.0],
        energy_gradient: grad,
        raw_pump_logit: Some(o_pump),
        ..still_ctx
    };
    let p_neural = pump_neural
        .plan(&grid, step, 2.0, false, Some(&emission), &ctx_neural)
        .expect("pump neural");
    let want_x = 4.0 + delta * tanh(o_pump).expect("tanh") * expected_axis_x;
    assert!((p_neural.deposit_position[0] - want_x).abs() < 1e-12);
}

#[test]
fn emission_v2_toggles_disabled_match_baseline_bit_exact() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let v1_cfg =
        ValidatedConfig::from_toml(&ecology_toml("field-ecology-v1", small_overrides(), ""))
            .expect("v1");
    let v2_cfg = ValidatedConfig::from_toml_with_overrides(
        &ecology_toml("field-ecology-v2", small_overrides(), ""),
        &[FeatureOverride::V1Compat],
    )
    .expect("v2 compat");

    let mut w1 = World::found(&v1_cfg, Arrangement::Compact, profile).expect("w1");
    let mut w2 = World::found(&v2_cfg, Arrangement::Compact, profile).expect("w2");
    assert_eq!(
        w1.footprint().state,
        w2.footprint().state,
        "disabled excitable-relay must reserve 0 extra population column bytes"
    );
    for _ in 0..48 {
        assert!(matches!(w1.step().expect("w1"), Stepped::Published(_)));
        assert!(matches!(w2.step().expect("w2"), Stepped::Published(_)));
    }
    assert_eq!(w1.digest(), w2.digest());
}

#[test]
fn checkpoint_v3_and_c7_c8_preserve_excitable_readiness() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    let toml = ecology_toml(
        "field-ecology-v2",
        small_overrides(),
        r#"
[v2]
profile = "minimal"

[v2.emission.surplus_floor]
enabled = true
energy_floor = 0.5

[v2.emission.excitable]
enabled = true
mode = "vesicle-readiness-v1"
fire_readiness_threshold = 0.5
depletion_fraction = 0.85
burst_rate_multiplier = 5.0
recovery_time_seconds = 1.0

[v2.emission.offset]
enabled = true
mode = "wake-velocity-v1"
distance_world_units = 0.25
"#,
    );
    let cfg = ValidatedConfig::from_toml(&toml).expect("valid v2 emission config");
    let mut compact = World::found(&cfg, Arrangement::Compact, profile).expect("compact");
    let mut freelist = World::found(&cfg, Arrangement::FreeList, profile).expect("freelist");

    // Verify all founders initialize secretory_readiness == 0.2 identically.
    assert!(
        compact
            .population()
            .iter()
            .all(|o| o.secretory_readiness.to_bits() == INITIAL_SECRETORY_READINESS.to_bits())
    );

    let footprint_before = compact.footprint();
    let introduce = Intervention::IntroduceOrganisms {
        cell: [16, 16],
        count: 4,
        energy: 2.5,
    };

    for tick in 0..64 {
        if tick == 32 {
            let before = compact.digest();
            if let Advance::Ready(cand) = compact.advance().expect("advance") {
                cand.cancel();
            }
            assert_eq!(compact.digest(), before, "C.8 cancel");
        }
        let ivs: &[Intervention] = if tick == 20 { &[introduce] } else { &[] };
        let Stepped::Published(r1) = compact.step_with(ivs).expect("c") else {
            panic!("pause");
        };
        assert!(matches!(
            freelist.step_with(ivs).expect("f"),
            Stepped::Published(_)
        ));
        if tick == 20 {
            assert_eq!(r1.introductions.len(), 4);
            for intro in &r1.introductions {
                let org = compact.population().get(intro.organism).expect("intro org");
                assert_eq!(
                    org.secretory_readiness.to_bits(),
                    INITIAL_SECRETORY_READINESS.to_bits(),
                    "introduced organism must initialize secretory_readiness = 0.2"
                );
            }
        }
    }
    assert_eq!(
        compact.digest(),
        freelist.digest(),
        "C.7 Compact vs FreeList must match bit-for-bit with excitable state"
    );
    // Verify readiness has actually evolved away from 0.2 mid-refractory!
    assert!(
        compact
            .population()
            .iter()
            .any(|o| o.secretory_readiness.to_bits() != INITIAL_SECRETORY_READINESS.to_bits()),
        "secretory_readiness should evolve away from initial 0.2 by tick 64"
    );
    // INV-15: footprint never grows while stepping.
    let footprint_after = compact.footprint();
    assert_eq!(footprint_after.scratch, footprint_before.scratch);
    assert!(footprint_after.state <= footprint_before.state);

    // Checkpoint v3 save at tick 64 -> restore -> step to 128 must match bit-for-bit!
    let mut writer = CheckpointWriter::new(&cfg.config().model);
    compact.write_checkpoint(&mut writer).expect("write");
    let bytes = writer.finish().expect("finish");
    let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("open");
    let mut restored = World::read_checkpoint(&reader, profile).expect("restore");
    assert_eq!(restored.digest(), compact.digest());
    for _ in 0..64 {
        assert!(matches!(compact.step().expect("c2"), Stepped::Published(_)));
        assert!(matches!(
            restored.step().expect("r2"),
            Stepped::Published(_)
        ));
    }
    assert_eq!(
        restored.digest(),
        compact.digest(),
        "checkpoint v3 must preserve secretory_readiness bit-for-bit"
    );
}

#[test]
fn e2e_stage_w_high_emission_survives_and_pulses() {
    let profile = NumericalProfile::reference_f32().expect("profile");
    // High-r_max (4x baseline 0.5 -> 2.0 / s) Stage-W signaling regime with
    // surplus-emission + excitable-relay + wake-offset enabled.
    let toml = ecology_toml(
        "field-ecology-v2",
        &[
            ("world", "width", "16.0"),
            ("world", "height", "16.0"),
            ("world", "grid_width", "32"),
            ("world", "grid_height", "32"),
            ("world", "tick_numerator", "1"),
            ("world", "tick_denominator", "32"),
            ("population", "initial_count", "64"),
            ("population", "maximum_count", "512"),
            ("population", "initial_energy", "3.0"),
            (
                "population",
                "capacity_policy",
                "\"refuse-excess-births-v1\"",
            ),
            ("sources", "count", "4"),
            ("sources", "power_per_source", "240.0"),
            ("sources", "sigma_world_units", "3.0"),
            ("sources", "path_direction_interval_seconds", "2.0"),
            ("chemistry", "uptake_capacity_per_second", "8.0"),
            ("costs", "basal_energy_per_second", "0.5"),
            ("emission", "maximum_fractional_rate_per_second", "2.0"),
            (
                "reproduction",
                "minimum_parent_energy_before_attempt",
                "2.0",
            ),
            ("reproduction", "maximum_attempt_rate_per_second", "4.0"),
            ("reproduction", "cooldown_seconds", "0.25"),
            ("execution", "command_durability", "\"volatile-in-memory\""),
        ],
        r#"
[v2]
profile = "minimal"

[v2.emission.surplus_floor]
enabled = true
energy_floor = 0.8

[v2.emission.excitable]
enabled = true
mode = "vesicle-readiness-v1"
fire_readiness_threshold = 0.5
depletion_fraction = 0.85
burst_rate_multiplier = 4.0
recovery_time_seconds = 1.0

[v2.emission.offset]
enabled = true
mode = "wake-velocity-v1"
distance_world_units = 0.25
"#,
    );
    let cfg = ValidatedConfig::from_toml(&toml).expect("valid stage W config");
    let mut world = World::found(&cfg, Arrangement::Compact, profile).expect("world");

    let mut total_emitted = 0_u64;
    let mut total_organism_ticks = 0_u64;
    let mut max_rel_residual = 0.0_f64;
    for _ in 0..1024 {
        let Stepped::Published(report) = world.step().expect("step") else {
            panic!("unexpected pause");
        };
        total_emitted += report.emitted as u64;
        total_organism_ticks += report.live as u64;
        let rel = report.closure.residual().abs() / report.closure.scale().max(1.0);
        max_rel_residual = max_rel_residual.max(rel);
    }
    let live = world.population().len();
    let emitting_fraction = if total_organism_ticks > 0 {
        total_emitted as f64 / total_organism_ticks as f64
    } else {
        0.0
    };
    let ledger_report = world.ledger().report();
    let cum_norm = ledger_report.normalized.unwrap_or(0.0);
    eprintln!(
        "Stage-W high-r_max E2E (1024 ticks): live={live}, emitting_fraction={emitting_fraction:.4}, max_rel_residual={max_rel_residual:e}, cum_norm={cum_norm:e}"
    );
    assert!(
        live > 0,
        "population must survive 4x r_max under surplus-emission"
    );
    assert!(
        total_emitted > 0,
        "excitable relay must actually fire bursts"
    );
    assert!(
        emitting_fraction < 0.25,
        "excitable relay must keep emission pulsatile (< 25%/tick), got {emitting_fraction:.4}"
    );
    assert!(
        max_rel_residual <= 1e-6,
        "INV-03 per-tick relative residual {max_rel_residual:e} exceeded 1e-6 (§21.4)"
    );
    assert!(
        cum_norm <= profile.residual.cumulative(),
        "INV-03 cumulative normalized residual {cum_norm:e} exceeded {}",
        profile.residual.cumulative()
    );
}
