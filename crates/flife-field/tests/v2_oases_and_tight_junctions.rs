//! Wave-1 `flife-field` evidence:
//! - `V2-02` (`oases`): phase-staggered ephemeral oasis resource landscapes,
//!   constant global power, zero-power skip, `M = 2` validation guard,
//!   heterogeneous `σ_j` resolution invariance & zero steady-state allocation.
//! - `V2-09` (`tight-junctions`): passive symmetric flux-form tight-junction
//!   diffusion, exact pre-decay conservation (`INV-01`/`02`/`03`), closed-ring
//!   lumen retention (`>= 10x`), localized puncture leak (`> 5x`), bit-exact
//!   worker/sub-range determinism, and zero allocation when disabled.
//! - Wave-1 `settle.rs` (`V2-03`/`V2-04`/`V2-05`) and `FieldBackend::advance_v2`
//!   additive seam contracts (exact `v1` delegation + clean `MissingCapability`
//!   refusal when v2 features are active).

use std::num::NonZeroUsize;

use flife_core::ids::SourceId;
use flife_core::numerics::{normalize_turns, sin_cos_turns};
use flife_core::rng::SemanticRng;
use flife_core::{CoreError, Tick, TimeStep, ValidatedConfig};
use flife_field::state::CHANNELS;
use flife_field::transport::transport;
use flife_field::{
    BarrierMode, BufferLimits, Cell, ChemistryConstants, ChemistryOrganism, ChemistryOutcome,
    ChemistryPacket, ConeTolerance, DepositSummary, Deposits, EmissionOrganism, EmissionOutcome,
    FIELD_PLANES, FieldBackend, FieldGrid, FieldPair, FieldScan, FieldState, FieldStepOptions,
    FieldStepReport, FieldWork, FluxEdges, Grants, MOTION_TERMS, MotionConstants, MotionOrganism,
    MotionOutcome, MotionV2Extras, MotorSteeringConstants, MotorSteeringInputs, MutationBirth,
    MutationConstants, MutationCounts, MutationValues, OasisSchedule, ProposalConstants,
    ProposalOrganism, ProposalOutcome, SettleBackend, Source, SourceKernel, TightJunctions,
    TransportPlan, TransportScratch, advance_field_on, advance_field_v2_on, count_cell_occupancy,
    source_sigma, transport_flux, transport_flux_on, transport_flux_range_on,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn appendix_a_toml() -> String {
    let appendix = &PLAN[PLAN.find("## Appendix A").expect("Appendix A heading")..];
    let start = appendix.find("```toml\n").expect("Appendix A TOML block") + "```toml\n".len();
    let length = appendix[start..].find("```").expect("closing fence");
    appendix[start..start + length].to_owned()
}

fn v2_toml(extra: &str) -> String {
    let base = appendix_a_toml().replace(
        "model = \"field-ecology-v1\"",
        "model = \"field-ecology-v2\"",
    );
    format!("{base}\n{extra}\n")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn grid(cells_x: u32, cells_y: u32, width: f64, height: f64) -> FieldGrid {
    FieldGrid::new(cells_x, cells_y, width, height).expect("grid")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn zeroed(grid: FieldGrid) -> FieldState {
    FieldState::zeroed(grid, &BufferLimits::host()).expect("allocate")
}

fn unit_channel(channel: usize) -> [f64; CHANNELS] {
    let mut dir = [0.0; CHANNELS];
    dir[channel] = 1.0;
    dir
}

// --------------------------------------------------------------- V2-02 oases

#[test]
fn oasis_envelope_constant_total_power_and_zero_skip() {
    let step = TimeStep::new(1, 32).expect("step");
    let base_power = 40.0;
    let period = 24.0;
    let schedule = OasisSchedule::new(period, 0.0, 2.0, 3.5).expect("schedule");
    let dt = f64::from(step.numerator()) / f64::from(step.denominator());
    let mut powers = Vec::new();

    for count in [3_usize, 4, 6, 8] {
        let mut saw_exact_zero = false;
        let mut saw_peak = false;
        let ticks = 1_536_u64; // 2 full 24s periods at 32 Hz.
        for tick in 0..ticks {
            schedule
                .powers_at(Tick::new(tick), step, count, base_power, &mut powers)
                .expect("powers");
            assert_eq!(powers.len(), count);
            // Independent oracle check against host-independent numerics.
            let t = (tick as f64 + 0.5) * dt;
            let mut oracle_w = Vec::with_capacity(count);
            for j in 0..count {
                let u = (t / period) + (j as f64 / count as f64);
                let phase = normalize_turns(u).expect("phase");
                let (s, _) = sin_cos_turns(phase).expect("sin");
                let pos = s.max(0.0);
                oracle_w.push(pos * pos);
            }
            let sum_w: f64 = oracle_w.iter().sum();
            assert!(sum_w > 0.0, "M={count} tick={tick} must keep Σw > 0");
            let total: f64 = powers.iter().sum();
            let expected_total = (count as f64) * base_power;
            assert!(
                (total - expected_total).abs() <= 1.0e-11 * expected_total,
                "M={count} tick={tick}: total={total} vs {expected_total}"
            );
            for (&got, &w) in powers.iter().zip(&oracle_w) {
                assert!(got.is_finite() && got >= 0.0);
                if w == 0.0 {
                    assert_eq!(got, 0.0);
                    saw_exact_zero = true;
                }
                if got > 1.5 * base_power {
                    saw_peak = true;
                }
            }
        }
        assert!(
            saw_exact_zero,
            "M={count} must experience exact zero-power dormant phases"
        );
        assert!(saw_peak, "M={count} must experience bright oasis blooms");
    }

    // Exercise sequential and fused advance_field_v2_on across zero-power &
    // peak ticks; verify zero-power sources report exact 0 and cone/ledger hold.
    let g = grid(32, 32, 32.0, 32.0);
    let plan = TransportPlan::new(&g, step, 0.2, 0.01).expect("plan");
    let limits = BufferLimits::host();
    let one = NonZeroUsize::new(1).expect("1");
    for fused in [false, true] {
        let base_kernel = if fused {
            SourceKernel::new_fused(2.5, 3.0).expect("fused")
        } else {
            SourceKernel::new(2.5, 3.0).expect("seq")
        };
        let mut sources: Vec<Source> = (0_u32..4)
            .map(|i| {
                Source::new(
                    SourceId::new(i),
                    [4.0 + 7.0 * f64::from(i), 16.0],
                    unit_channel(i as usize),
                    base_power,
                )
                .expect("source")
            })
            .collect();
        let mut pair = FieldPair::zeroed(g, &limits).expect("pair");
        let mut work = FieldWork::new(&g, &limits, one).expect("work");
        work.reserve_supports(sources.len(), &base_kernel)
            .expect("reserve");
        let mut scratch = Vec::new();
        for tick in [0_u64, 64, 192, 384, 576] {
            schedule
                .apply_powers(
                    Tick::new(tick),
                    step,
                    base_power,
                    &mut sources,
                    &mut scratch,
                )
                .expect("apply_powers");
            let report = advance_field_v2_on(
                &mut pair,
                &plan,
                &base_kernel,
                &sources,
                step,
                FieldStepOptions::BASELINE,
                &mut work,
            )
            .expect("advance");
            pair.commit();
            let expected_injected = (sources.len() as f64) * base_power * dt;
            assert!(
                (report.injected_declared - expected_injected).abs() <= 1.0e-12,
                "fused={fused} tick={tick}: declared={}",
                report.injected_declared
            );
            assert!(
                report.residual.abs() <= 1.0e-5 * (report.energy_after + expected_injected + 1.0),
                "fused={fused} tick={tick}: residual={}",
                report.residual
            );
            let mut saw_zero = false;
            for (src, dep) in sources.iter().zip(&report.deposits) {
                if src.power() == 0.0 {
                    saw_zero = true;
                    assert_eq!(dep.support_cells, 0);
                    assert_eq!(dep.declared, 0.0);
                    assert_eq!(dep.normalized_total, 0.0);
                    assert_eq!(dep.applied, 0.0);
                } else {
                    assert!(dep.support_cells > 0);
                    assert!((dep.normalized_total - dep.declared).abs() <= 1.0e-11);
                }
            }
            assert!(
                saw_zero,
                "tick={tick} with M=4 and floor=0 must have an inactive oasis"
            );
            pair.current()
                .check_invariants(
                    ConeTolerance::new(f64::from(f32::MIN_POSITIVE), 1.0e-6).expect("tol"),
                )
                .expect("INV-01 and INV-02 hold");
        }
    }
}

#[test]
fn oasis_m2_zero_min_frac_rejected_by_validate_and_defensive_guard() {
    let bad = v2_toml(
        "[v2]\nprofile = \"supercolony\"\n[v2.oases]\nenabled = true\nminimum_power_fraction = 0.0",
    )
    .replace("[sources]\ncount = 8", "[sources]\ncount = 2");
    assert!(bad.contains("[sources]\ncount = 2"));
    let err = ValidatedConfig::from_toml(&bad).expect_err("M=2 with floor=0 must fail validate");
    assert!(
        matches!(err, CoreError::InvalidConfig(_)),
        "expected InvalidConfig, got {err:?}"
    );

    let ok_floor = v2_toml(
        "[v2]\nprofile = \"supercolony\"\n[v2.oases]\nenabled = true\nminimum_power_fraction = 0.05",
    )
    .replace("[sources]\ncount = 8", "[sources]\ncount = 2");
    assert!(ok_floor.contains("[sources]\ncount = 2"));
    let valid_floor = ValidatedConfig::from_toml(&ok_floor).expect("M=2 with floor=0.05 valid");
    assert!(
        OasisSchedule::from_config(&valid_floor)
            .expect("from_config")
            .is_some()
    );

    let ok_m3 = v2_toml(
        "[v2]\nprofile = \"supercolony\"\n[v2.oases]\nenabled = true\nminimum_power_fraction = 0.0",
    )
    .replace("[sources]\ncount = 8", "[sources]\ncount = 3");
    assert!(ok_m3.contains("[sources]\ncount = 3"));
    let valid_m3 = ValidatedConfig::from_toml(&ok_m3).expect("M=3 with floor=0 valid");
    assert!(
        OasisSchedule::from_config(&valid_m3)
            .expect("from_config")
            .is_some()
    );

    // Direct defensive check on powers_at with M=2 and floor=0: never NaN.
    let schedule = OasisSchedule::new(2.0, 0.0, 2.0, 2.0).expect("schedule");
    let step = TimeStep::new(1, 2).expect("dt=0.5");
    let mut powers = Vec::new();
    for tick in 0..16 {
        schedule
            .powers_at(Tick::new(tick), step, 2, 10.0, &mut powers)
            .expect("defensive guard");
        assert!(powers.iter().all(|p| p.is_finite() && *p >= 0.0));
    }
}

#[test]
fn oasis_disabled_matches_constant_sources_bit_exact() {
    let v1 = ValidatedConfig::from_toml(&appendix_a_toml()).expect("v1");
    assert_eq!(OasisSchedule::from_config(&v1).expect("v1"), None);

    let disabled_toml = v2_toml("[v2]\nprofile = \"supercolony\"\n[v2.oases]\nenabled = false");
    let disabled = ValidatedConfig::from_toml(&disabled_toml).expect("v2 disabled");
    assert_eq!(
        OasisSchedule::from_config(&disabled).expect("disabled"),
        None
    );

    // Bit-exact Phase 2 parity with `advance_field_on`.
    let g = grid(32, 32, 32.0, 32.0);
    let step = TimeStep::new(1, 32).expect("step");
    let plan = TransportPlan::new(&g, step, 0.25, 0.02).expect("plan");
    let kernel = SourceKernel::new(3.0, 4.0).expect("kernel");
    let sources: Vec<Source> = (0_u32..4)
        .map(|i| {
            Source::new(
                SourceId::new(i),
                [5.0 + 6.0 * f64::from(i), 12.0 + f64::from(i)],
                unit_channel(i as usize),
                20.0,
            )
            .expect("source")
        })
        .collect();
    let limits = BufferLimits::host();
    let two = NonZeroUsize::new(2).expect("2");
    let mut pair_base = FieldPair::zeroed(g, &limits).expect("base");
    let mut pair_v2 = FieldPair::zeroed(g, &limits).expect("v2");
    let mut work_base = FieldWork::new(&g, &limits, two).expect("work base");
    let mut work_v2 = FieldWork::new(&g, &limits, two).expect("work v2");
    work_base
        .reserve_supports(sources.len(), &kernel)
        .expect("reserve");
    work_v2
        .reserve_supports(sources.len(), &kernel)
        .expect("reserve");

    for _ in 0..8 {
        let r_base = advance_field_on(
            &mut pair_base,
            &plan,
            &kernel,
            &sources,
            step,
            &mut work_base,
        )
        .expect("base");
        let r_v2 = advance_field_v2_on(
            &mut pair_v2,
            &plan,
            &kernel,
            &sources,
            step,
            FieldStepOptions::BASELINE,
            &mut work_v2,
        )
        .expect("v2");
        pair_base.commit();
        pair_v2.commit();
        assert_eq!(r_base, r_v2);
        let b_bits: Vec<u32> = pair_base
            .current()
            .as_slice()
            .iter()
            .map(|v| v.to_bits())
            .collect();
        let v_bits: Vec<u32> = pair_v2
            .current()
            .as_slice()
            .iter()
            .map(|v| v.to_bits())
            .collect();
        assert_eq!(b_bits, v_bits);
    }
}

#[test]
fn heterogeneous_sigma_resolution_invariance_and_worker_determinism() {
    let step = TimeStep::new(1, 32).expect("step");
    let rng = SemanticRng::new(42);
    let schedule = OasisSchedule::new(24.0, 0.1, 1.5, 4.0).expect("schedule");
    let s0 = source_sigma(&rng, SourceId::new(0), 1.5, 4.0).expect("s0");
    let s1 = source_sigma(&rng, SourceId::new(1), 1.5, 4.0).expect("s1");
    assert!((1.5..=4.0).contains(&s0) && (1.5..=4.0).contains(&s1));
    assert_ne!(s0, s1, "distinct source IDs draw distinct widths");
    assert_eq!(
        source_sigma(&rng, SourceId::new(0), 2.5, 2.5).expect("equal"),
        2.5
    );

    let base_kernel = SourceKernel::new_fused(2.0, 3.0).expect("fused");
    let kernels = [
        base_kernel.with_sigma(1.5).expect("1.5"),
        base_kernel.with_sigma(4.0).expect("4.0"),
    ];
    let sources = [
        Source::new(SourceId::new(0), [8.25, 8.75], unit_channel(0), 32.0).expect("src0"),
        Source::new(SourceId::new(1), [22.5, 21.0], unit_channel(1), 32.0).expect("src1"),
    ];
    let limits = BufferLimits::host();
    let one = NonZeroUsize::new(1).expect("1");
    let four = NonZeroUsize::new(4).expect("4");

    // Resolution invariance across 64² vs 128² at fixed 32x32 world size.
    let mut totals = Vec::new();
    for cells in [64_u32, 128] {
        let g = grid(cells, cells, 32.0, 32.0);
        let plan = TransportPlan::new(&g, step, 0.0, 0.0).expect("plan");
        let mut pair_1 = FieldPair::zeroed(g, &limits).expect("p1");
        let mut pair_4 = FieldPair::zeroed(g, &limits).expect("p4");
        let mut work_1 = FieldWork::new(&g, &limits, one).expect("w1");
        let mut work_4 = FieldWork::new(&g, &limits, four).expect("w4");
        work_1.reserve_supports_for_kernels(&kernels).expect("res1");
        work_4.reserve_supports_for_kernels(&kernels).expect("res4");
        let bytes_before = work_4.allocated_bytes();
        let options = FieldStepOptions {
            source_kernels: Some(&kernels),
            flux_edges: None,
        };
        let r1 = advance_field_v2_on(
            &mut pair_1,
            &plan,
            &base_kernel,
            &sources,
            step,
            options,
            &mut work_1,
        )
        .expect("step 1");
        let r4 = advance_field_v2_on(
            &mut pair_4,
            &plan,
            &base_kernel,
            &sources,
            step,
            options,
            &mut work_4,
        )
        .expect("step 4");
        assert_eq!(
            work_4.allocated_bytes(),
            bytes_before,
            "steady-state heterogeneous oasis step must allocate 0 bytes"
        );
        pair_1.commit();
        pair_4.commit();
        assert_eq!(r1, r4);
        let bits_1: Vec<u32> = pair_1
            .current()
            .as_slice()
            .iter()
            .map(|v| v.to_bits())
            .collect();
        let bits_4: Vec<u32> = pair_4
            .current()
            .as_slice()
            .iter()
            .map(|v| v.to_bits())
            .collect();
        assert_eq!(bits_1, bits_4, "{cells}² 1 vs 4 workers bit-exact");
        assert_eq!(r1.injected_declared, 2.0);
        assert!((r1.energy_after - 2.0).abs() <= 1.0e-5);
        totals.push(r1.energy_after);
    }
    assert!(
        (totals[0] - totals[1]).abs() <= 1.0e-5,
        "heterogeneous σ_j total power must be resolution invariant: {totals:?}"
    );

    // Seed kernels helper preserves mode and draws inside [sigma_min, sigma_max].
    let seeded = schedule
        .seed_kernels(&rng, &base_kernel, &sources)
        .expect("seed_kernels");
    assert_eq!(seeded.len(), 2);
    assert!(
        seeded
            .iter()
            .all(|k| k.is_fused() && k.cutoff_sigma() == 3.0)
    );
}

// ------------------------------------------------------ V2-09 tight junctions

fn ring_occupancy(
    width: usize,
    height: usize,
    cx: i32,
    cy: i32,
    r_min2: i32,
    r_max2: i32,
    density: u32,
) -> (Vec<u32>, Vec<usize>) {
    let mut occ = vec![0_u32; width * height];
    let mut lumen = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let dx = x as i32 - cx;
            let dy = y as i32 - cy;
            let d2 = dx * dx + dy * dy;
            if d2 < r_min2 {
                lumen.push(y * width + x);
            } else if d2 <= r_max2 {
                occ[y * width + x] = density;
            }
        }
    }
    (occ, lumen)
}

#[test]
fn flux_stencil_exact_conservation_and_cone_on_sharp_ring() {
    let g = grid(32, 32, 32.0, 32.0);
    let step = TimeStep::new(1, 32).expect("step");
    let plan = TransportPlan::new(&g, step, 0.16, 0.0).expect("plan");
    let limits = BufferLimits::host();
    let (occ, lumen) = ring_occupancy(32, 32, 16, 16, 25, 36, 5);
    assert!(!lumen.is_empty());
    let exterior: Vec<usize> = (0..(32 * 32))
        .filter(|&idx| !lumen.contains(&idx) && occ[idx] == 0)
        .collect();
    assert!(!exterior.is_empty());

    let init_field = || {
        let mut state = zeroed(g);
        let mut inside = Cell::EMPTY;
        inside.energy = 10.0;
        inside.moments[0] = 10.0;
        let mut outside = Cell::EMPTY;
        outside.energy = 10.0;
        outside.moments[1] = 10.0;
        for y in 0..32_u32 {
            for x in 0..32_u32 {
                let idx = (y as usize) * 32 + (x as usize);
                let cell = if lumen.contains(&idx) {
                    &inside
                } else {
                    &outside
                };
                state.set_cell(x, y, cell).expect("set");
            }
        }
        state
    };

    // Baseline uniform diffusion (no tight junctions): measure transmembrane m_0
    // flux crossing the ring into the exterior.
    let mut base_cur = init_field();
    let mut base_next = zeroed(g);
    let mut scratch = TransportScratch::new(&g, &limits).expect("scratch");
    for _ in 0..200 {
        transport(&plan, &base_cur, &mut base_next, &mut scratch).expect("base");
        std::mem::swap(&mut base_cur, &mut base_next);
    }
    let base_transmembrane: f64 = exterior
        .iter()
        .map(|&idx| f64::from(base_cur.moment(0).expect("m0")[idx]))
        .sum();
    assert!(base_transmembrane > 1.0);

    // Both HarmonicV1 and MinV1 tight junctions must conserve energy (< 1e-6),
    // preserve the cone, and reduce transmembrane flux by >= 10x!
    for mode in [BarrierMode::HarmonicV1, BarrierMode::MinV1] {
        let barrier = TightJunctions::new(mode, 0.04, 2.0).expect("barrier");
        let mut edges = FluxEdges::new(&g, &limits).expect("edges");
        barrier.fill_edges(&plan, &occ, &mut edges).expect("fill");
        let mut cur = init_field();
        let mut next = zeroed(g);
        let energy_initial = cur.energy_total();
        for _ in 0..200 {
            transport_flux(&plan, &edges, &cur, &mut next, &mut scratch).expect("flux");
            std::mem::swap(&mut cur, &mut next);
        }
        let energy_final = cur.energy_total();
        let rel_drift = (energy_final - energy_initial).abs() / energy_initial;
        assert!(
            rel_drift < 1.0e-6,
            "{mode:?}: pre-decay conservation drift {rel_drift:e} exceeds 1e-6"
        );
        cur.check_invariants(
            ConeTolerance::new(f64::from(f32::MIN_POSITIVE), 1.0e-6).expect("tol"),
        )
        .expect("INV-01 and INV-02 hold");

        let tj_transmembrane: f64 = exterior
            .iter()
            .map(|&idx| f64::from(cur.moment(0).expect("m0")[idx]))
            .sum();
        let retention_ratio = base_transmembrane / tj_transmembrane.max(1.0e-12);
        assert!(
            retention_ratio >= 10.0,
            "{mode:?}: expected >= 10x transmembrane flux reduction, got {retention_ratio:.2}x \
             (base_transmembrane={base_transmembrane:.3}, tj_transmembrane={tj_transmembrane:.3})"
        );
    }
}

#[test]
fn flux_stencil_breach_causes_localized_leak() {
    let g = grid(32, 32, 32.0, 32.0);
    let step = TimeStep::new(1, 32).expect("step");
    let plan = TransportPlan::new(&g, step, 1.6, 0.0).expect("plan");
    let limits = BufferLimits::host();
    let barrier = TightJunctions::new(BarrierMode::HarmonicV1, 0.04, 1.2).expect("barrier");

    // Ring around (16, 16) with high density wall; lumen holds energy 10,
    // exterior holds energy 0.
    let (occ_intact, lumen) = ring_occupancy(32, 32, 16, 16, 25, 36, 20);
    let mut occ_breach = occ_intact.clone();
    // Puncture 2 adjacent ring cells on the east side: (21, 16) and (22, 16).
    occ_breach[16 * 32 + 21] = 0;
    occ_breach[16 * 32 + 22] = 0;
    // Control: zero 2 interior non-ring cells far from the boundary: (16, 16) and (17, 16).
    let mut occ_interior = occ_intact;
    occ_interior[16 * 32 + 16] = 0;
    occ_interior[16 * 32 + 17] = 0;

    let run_leak = |occ: &[u32]| {
        let mut edges = FluxEdges::new(&g, &limits).expect("edges");
        barrier.fill_edges(&plan, occ, &mut edges).expect("fill");
        let mut cur = zeroed(g);
        let mut next = zeroed(g);
        let mut scratch = TransportScratch::new(&g, &limits).expect("scratch");
        let mut inside = Cell::EMPTY;
        inside.energy = 10.0;
        for &idx in &lumen {
            let x = (idx % 32) as u32;
            let y = (idx / 32) as u32;
            cur.set_cell(x, y, &inside).expect("set");
        }
        for _ in 0..40 {
            transport_flux(&plan, &edges, &cur, &mut next, &mut scratch).expect("flux");
            std::mem::swap(&mut cur, &mut next);
        }
        // Measure energy accumulated right outside the east breach site.
        f64::from(cur.energy()[16 * 32 + 23]) + f64::from(cur.energy()[16 * 32 + 24])
    };

    let leak_breach = run_leak(&occ_breach);
    let leak_interior = run_leak(&occ_interior);
    let ratio = leak_breach / leak_interior.max(1.0e-12);
    assert!(
        ratio > 5.0,
        "puncturing 2 ring cells must cause > 5x localized leak vs interior cut: \
         ratio={ratio:.2}x (breach={leak_breach:.6}, interior={leak_interior:.6})"
    );
}

#[test]
fn flux_stencil_disabled_zero_alloc_worker_and_plane_range_bit_exact() {
    let v1 = ValidatedConfig::from_toml(&appendix_a_toml()).expect("v1");
    assert_eq!(TightJunctions::from_config(&v1).expect("v1"), None);

    let disabled_toml = v2_toml("[v2]\nprofile = \"supercolony\"\n[v2.barrier]\nenabled = false");
    let disabled = ValidatedConfig::from_toml(&disabled_toml).expect("v2 disabled");
    assert_eq!(
        TightJunctions::from_config(&disabled).expect("disabled"),
        None
    );

    let g = grid(24, 24, 24.0, 24.0);
    let step = TimeStep::new(1, 32).expect("step");
    let plan = TransportPlan::new(&g, step, 0.8, 0.05).expect("plan");
    let limits = BufferLimits::host();
    let one = NonZeroUsize::new(1).expect("1");
    let four = NonZeroUsize::new(4).expect("4");

    // Disabled FieldWork holds 0 B flux scratch; reserve_flux adds exact occupancy + edges bytes.
    let mut work = FieldWork::new(&g, &limits, one).expect("work");
    let bytes_disabled = work.allocated_bytes();
    assert!(work.flux_edges().is_none());
    assert!(work.cell_occupancy().is_empty());
    work.reserve_flux().expect("reserve_flux");
    let bytes_reserved = work.allocated_bytes();
    let expected_flux_bytes = g.cell_count() * (4 + 8 + 8);
    assert_eq!(bytes_reserved - bytes_disabled, expected_flux_bytes);

    // Steady-state prepare_tight_junctions allocates 0 extra bytes.
    let barrier = TightJunctions::new(BarrierMode::HarmonicV1, 0.04, 1.2).expect("barrier");
    let positions: Vec<[f64; 2]> = (0..64)
        .map(|i| [f64::from(i % 24) + 0.25, f64::from((i * 5) % 24) + 0.75])
        .collect();
    let mut occ = Vec::new();
    count_cell_occupancy(&g, &positions, &mut occ).expect("count");
    let edges = work
        .prepare_tight_junctions(&barrier, &plan, &positions)
        .expect("prepare")
        .clone();
    assert_eq!(work.cell_occupancy(), occ.as_slice());
    assert_eq!(work.allocated_bytes(), bytes_reserved);

    // Build heterogeneous field state and compare 1 vs 4 workers + split plane_range.
    let mut current = zeroed(g);
    for y in 0..24_u32 {
        for x in 0..24_u32 {
            let mut cell = Cell::EMPTY;
            cell.energy = 1.0 + ((x * 7 + y * 13) % 11) as f32 * 0.5;
            cell.moments[(x as usize + y as usize) % CHANNELS] = cell.energy * 0.5;
            current.set_cell(x, y, &cell).expect("set");
        }
    }
    let mut next_1 = zeroed(g);
    let mut next_4 = zeroed(g);
    let mut next_split = zeroed(g);
    let mut s1 = vec![TransportScratch::new(&g, &limits).expect("s1")];
    let mut s4: Vec<_> = (0..4)
        .map(|_| TransportScratch::new(&g, &limits).expect("s4"))
        .collect();
    let r1 = transport_flux_on(&plan, &edges, &current, &mut next_1, &mut s1).expect("1w");
    let r4 = transport_flux_on(&plan, &edges, &current, &mut next_4, &mut s4).expect("4w");
    assert_eq!(r1, r4);
    let bits_1: Vec<u32> = next_1.as_slice().iter().map(|v| v.to_bits()).collect();
    let bits_4: Vec<u32> = next_4.as_slice().iter().map(|v| v.to_bits()).collect();
    assert_eq!(bits_1, bits_4, "1 vs 4 workers must be bit-identical");

    // Split plane_range 0..25 and 25..33 (reusable by V2-15) must match 0..33 bit-exact.
    let r_a = transport_flux_range_on(
        &plan,
        &edges,
        &current,
        &mut next_split,
        0..25,
        Some(0),
        &mut s4,
    )
    .expect("range a");
    let r_b = transport_flux_range_on(
        &plan,
        &edges,
        &current,
        &mut next_split,
        25..FIELD_PLANES as usize,
        None,
        &mut s4,
    )
    .expect("range b");
    assert_eq!(r_a, Some(r1));
    assert_eq!(r_b, None);
    let bits_split: Vec<u32> = next_split.as_slice().iter().map(|v| v.to_bits()).collect();
    assert_eq!(
        bits_split, bits_1,
        "split plane_range must be bit-identical"
    );

    // Planted negative: conductance exceeding r_x must be refused without mutating next.
    let mut bad_edges = edges;
    bad_edges.edge_x_mut()[0] = plan.ratios().0 + 0.01;
    let mut untouched = zeroed(g);
    let before: Vec<u32> = untouched.as_slice().iter().map(|v| v.to_bits()).collect();
    let err = transport_flux_on(&plan, &bad_edges, &current, &mut untouched, &mut s1)
        .expect_err("conductance > r_x must fail");
    assert!(matches!(err, CoreError::NumericalInvalid { .. }));
    let after: Vec<u32> = untouched.as_slice().iter().map(|v| v.to_bits()).collect();
    assert_eq!(after, before);

    // Ensure 4-worker advance_field_v2_on composes V2-02 + V2-09 cleanly.
    let mut work4 = FieldWork::new(&g, &limits, four).expect("work4");
    work4.reserve_flux().expect("res_flux");
    let kernel = SourceKernel::new(2.0, 3.0).expect("k");
    work4.reserve_supports(1, &kernel).expect("res_sup");
    let bytes4 = work4.allocated_bytes();
    let edges4 = work4
        .prepare_tight_junctions(&barrier, &plan, &positions)
        .expect("prep")
        .clone();
    let mut pair = FieldPair::zeroed(g, &limits).expect("pair");
    let src = [Source::new(SourceId::new(0), [12.0, 12.0], unit_channel(0), 10.0).expect("s")];
    advance_field_v2_on(
        &mut pair,
        &plan,
        &kernel,
        &src,
        step,
        FieldStepOptions {
            source_kernels: None,
            flux_edges: Some(&edges4),
        },
        &mut work4,
    )
    .expect("composed step");
    assert_eq!(work4.allocated_bytes(), bytes4, "0 steady-state allocation");
}

// --------------------------------------------------- Wave-1 settle/field seams

#[derive(Debug, Default)]
struct V1OnlySettle {
    motion_calls: usize,
    emission_calls: usize,
}

impl SettleBackend for V1OnlySettle {
    fn describe(&self) -> String {
        "v1-only-test".into()
    }
    fn chemistry(
        &mut self,
        _: &FieldState,
        _: &ChemistryConstants,
        _: &[ChemistryOrganism],
        _: &[ChemistryPacket],
        _: &mut Vec<ChemistryOutcome>,
    ) -> Result<(), CoreError> {
        Ok(())
    }
    fn motion(
        &mut self,
        _: &MotionConstants,
        _: &[MotionOrganism],
        out: &mut Vec<MotionOutcome>,
    ) -> Result<(), CoreError> {
        self.motion_calls += 1;
        out.clear();
        Ok(())
    }
    fn emission(
        &mut self,
        _: &FieldGrid,
        _: f64,
        _: &[EmissionOrganism],
        out: &mut Vec<EmissionOutcome>,
    ) -> Result<(), CoreError> {
        self.emission_calls += 1;
        out.clear();
        Ok(())
    }
    fn proposal(
        &mut self,
        _: &ProposalConstants,
        _: &[ProposalOrganism],
        _: &mut Vec<ProposalOutcome>,
    ) -> Result<(), CoreError> {
        Ok(())
    }
    fn mutation(
        &mut self,
        _: &MutationConstants,
        _: &[MutationBirth],
        _: MutationValues<'_>,
        _: &mut Vec<MutationCounts>,
    ) -> Result<(), CoreError> {
        Ok(())
    }
}

#[derive(Debug, Default)]
struct V1OnlyField {
    advance_calls: usize,
}

impl FieldBackend for V1OnlyField {
    fn describe(&self) -> String {
        "v1-only-field".into()
    }
    fn advance(
        &mut self,
        pair: &mut FieldPair,
        plan: &TransportPlan,
        kernel: &SourceKernel,
        sources: &[Source],
        step: TimeStep,
    ) -> Result<FieldStepReport, CoreError> {
        self.advance_calls += 1;
        let mut work = FieldWork::new(plan.grid(), &BufferLimits::host(), NonZeroUsize::MIN)?;
        advance_field_on(pair, plan, kernel, sources, step, &mut work)
    }
    fn reduce(
        &mut self,
        _: &mut FieldState,
        _: &mut Grants,
        _: [&mut Deposits; 2],
    ) -> Result<[DepositSummary; 2], CoreError> {
        Ok([DepositSummary::default(), DepositSummary::default()])
    }
    fn check(&mut self, _: &FieldState, _: ConeTolerance) -> Result<FieldScan, CoreError> {
        Ok(FieldScan {
            offending: None,
            largest: None,
        })
    }
}

#[test]
fn wave1_settle_and_field_v2_seams_delegate_v1_and_refuse_active_v2() {
    let mut backend = V1OnlySettle::default();
    let constants = MotionConstants {
        step_seconds: 1.0 / 32.0,
        basal: 0.1,
        sensor: 0.01,
        sensor_unit_scale: 1.0,
        sensor_scale_exponent: 1.0,
        neural: 0.0,
        decay: 0.9,
        force_coefficient: 0.1,
        maximum_speed: 2.0,
        maximum_acceleration: 4.0,
        cost: 0.1,
        response_norm_limit: 1.0,
        uptake_capacity: 1.0,
        world_width: 16.0,
        world_height: 16.0,
    };
    let org = MotionOrganism {
        remaining: 1.0,
        exhausted: false,
        evaluated: true,
        parameters: 100,
        sensing_scale: 1.0,
        absorbed: 0.0,
        velocity: [0.0, 0.0],
        position: [1.0, 1.0],
        response: [0.0; MOTION_TERMS],
        gradients: [[0.0; 2]; MOTION_TERMS],
    };
    let mut m_out = Vec::new();
    // Exact v1 identity delegates to v1 `motion`.
    backend
        .motion_v2(
            &constants,
            None,
            &[org],
            &[MotionV2Extras::IDENTITY],
            &mut m_out,
        )
        .expect("v1 identity delegates");
    assert_eq!(backend.motion_calls, 1);

    // Active senescence (basal_multiplier > 1) refuses with MissingCapability.
    let senescent = MotionV2Extras {
        basal_multiplier: 1.5,
        motor: MotorSteeringInputs::default(),
    };
    assert!(matches!(
        backend.motion_v2(&constants, None, &[org], &[senescent], &mut m_out),
        Err(CoreError::MissingCapability { .. })
    ));

    // Active so2-motor refuses with MissingCapability.
    let steered = MotionV2Extras {
        basal_multiplier: 1.0,
        motor: MotorSteeringInputs {
            energy_gradient: 0.25,
            chemical_gradient: 0.0,
            chiral_contour: 0.0,
            heading_propulsion: 0.5,
        },
    };
    let motor_c = MotorSteeringConstants {
        energy_gradient_gain: 2.0,
        chemical_gradient_gain: 2.0,
        chiral_contour_gain: 1.0,
        heading_propulsion_fraction: 0.5,
        internal_energy_scale: 1.0,
    };
    assert!(matches!(
        backend.motion_v2(&constants, Some(&motor_c), &[org], &[steered], &mut m_out),
        Err(CoreError::MissingCapability { .. })
    ));

    // Invalid extras length or basal_multiplier < 1.0 refused cleanly.
    assert!(matches!(
        backend.motion_v2(&constants, None, &[org], &[], &mut m_out),
        Err(CoreError::LayoutMismatch { .. })
    ));
    assert!(matches!(
        backend.motion_v2(
            &constants,
            None,
            &[org],
            &[MotionV2Extras {
                basal_multiplier: 0.5,
                motor: MotorSteeringInputs::default(),
            }],
            &mut m_out,
        ),
        Err(CoreError::NumericalInvalid { .. })
    ));

    // Emission floor seam: 0.0 delegates to v1; > 0 refuses MissingCapability; < 0 invalid.
    let g = grid(8, 8, 8.0, 8.0);
    let mut e_out = Vec::new();
    backend
        .emission_with_floor(&g, 0.8, 0.0, &[], &mut e_out)
        .expect("zero floor delegates");
    assert_eq!(backend.emission_calls, 1);
    assert!(matches!(
        backend.emission_with_floor(&g, 0.8, 0.5, &[], &mut e_out),
        Err(CoreError::MissingCapability { .. })
    ));
    assert!(matches!(
        backend.emission_with_floor(&g, 0.8, -0.1, &[], &mut e_out),
        Err(CoreError::NumericalInvalid { .. })
    ));

    // FieldBackend::advance_v2 seam: baseline delegates, active v2 refuses MissingCapability.
    let mut f_backend = V1OnlyField::default();
    let step = TimeStep::new(1, 32).expect("step");
    let plan = TransportPlan::new(&g, step, 0.1, 0.0).expect("plan");
    let kernel = SourceKernel::new(1.5, 3.0).expect("kernel");
    let mut pair = FieldPair::zeroed(g, &BufferLimits::host()).expect("pair");
    let active_src = [Source::new(SourceId::new(0), [4.0, 4.0], unit_channel(0), 10.0).expect("s")];
    f_backend
        .advance_v2(
            &mut pair,
            &plan,
            &kernel,
            &active_src,
            step,
            FieldStepOptions::BASELINE,
        )
        .expect("baseline delegates");
    assert_eq!(f_backend.advance_calls, 1);

    let zero_src = [Source::new(SourceId::new(0), [4.0, 4.0], unit_channel(0), 0.0).expect("z")];
    assert!(matches!(
        f_backend.advance_v2(
            &mut pair,
            &plan,
            &kernel,
            &zero_src,
            step,
            FieldStepOptions::BASELINE,
        ),
        Err(CoreError::MissingCapability { .. })
    ));
    let edges = FluxEdges::new(&g, &BufferLimits::host()).expect("edges");
    assert!(matches!(
        f_backend.advance_v2(
            &mut pair,
            &plan,
            &kernel,
            &active_src,
            step,
            FieldStepOptions {
                source_kernels: None,
                flux_edges: Some(&edges),
            },
        ),
        Err(CoreError::MissingCapability { .. })
    ));
}
