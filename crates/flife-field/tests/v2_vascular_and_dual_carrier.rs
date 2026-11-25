//! Wave-2 `flife-field` evidence:
//! - `V2-14` (`vascular-field`): 4-neighbor flux-reinforced Murray / Physarum
//!   self-carving capillary network, exact pre-decay conservation & cone
//!   (`INV-01`/`02`/`03`), `C.8` cancel-safe ping-pong `FieldPair` storage,
//!   sharp bimodal channel carving at `μ = 1.5` vs diffuse sublinear control
//!   at `μ = 0.7`, overflow guard (`|F_e| = 1e35`, `1e200`, `0`), substep
//!   invariance, and `0 B` + bit-exact parity when disabled.
//! - `V2-15` (`dual-carrier-trails`, realizing `FL-055`): 34-plane two-carrier
//!   hybrid field (Carrier 0 `0..25` + Carrier 1 `25..34`), independent Lorentz
//!   cones (`check_invariants_on` / `scan_invariants` / `summarize_scan`),
//!   100x diffusivity / 20x decay separation, non-destructive Carrier-0-only
//!   `apply_grants_on` leaving Carrier 1 bit-for-bit untouched, sub-unit
//!   emission split (`split_dual_carrier_emission`), `apply_deposits_to_carrier_on`,
//!   34-plane `SensorPyramid` (`sample_dual` / `sample_dual_batch`),
//!   full 4-feature Wave-1+Wave-2 composition (`V2-02` + `V2-09` + `V2-14` +
//!   `V2-15`) with `0 B` steady-state allocation, and `0 B` + 33-plane
//!   bit-exact parity when disabled.

use std::num::NonZeroUsize;

use flife_core::ids::{OrganismId, SourceId};
use flife_core::{CoreError, TimeStep, ValidatedConfig};
use flife_field::state::CHANNELS;
use flife_field::{
    Arrangement, BarrierMode, BufferLimits, CARRIER0_MOMENT_CHANNELS, CARRIER1_ENERGY_PLANE,
    CARRIER1_FIRST_MOMENT_PLANE, CARRIER1_MOMENT_CHANNELS, CarrierLayout, Cell, ConeTolerance,
    DUAL_CARRIER_PLANES, Demand, DepositSummary, Deposits, DualCarrierRules, DualRawSample,
    FIELD_PLANES, FieldBackend, FieldFootprint, FieldGrid, FieldLayout, FieldPair, FieldScan,
    FieldState, FieldStepPhase2, FieldStepReport, FieldWork, FluxEdges, Grants, MAX_PLANES,
    OasisSchedule, RawSample, SensorBackend, SensorPyramid, SensorQuery, Source, SourceKernel,
    TightJunctions, TransportPlan, TransportScratch, VASCULAR_INITIAL_CONDUCTANCE, VascularField,
    advance_field_on, advance_field_wave2_on, apply_deposits_to_carrier_on,
    field_planes_for_config, reduce_dual_carrier_field_on, relayout, resolve_uptake,
    split_dual_carrier_emission, transport_flux,
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

fn unit_channel(channel: usize) -> [f64; CHANNELS] {
    let mut dir = [0.0; CHANNELS];
    dir[channel] = 1.0;
    dir
}

// ------------------------------------------------------- V2-14 vascular-field

#[test]
fn vascular_exact_conservation_and_cone() {
    let g = grid(32, 32, 32.0, 32.0);
    let step = TimeStep::new(1, 32).expect("step");
    let limits = BufferLimits::host();
    let vascular = VascularField::new(0.025, 0.32, 3.0, 0.6, 0.05, 1.5).expect("vascular");
    let plan = vascular.transport_plan(&g, step, 0.0).expect("plan");
    let barrier = TightJunctions::new(BarrierMode::HarmonicV1, 0.04, 1.2).expect("barrier");

    let cells = g.cell_count() as usize;
    let gx: Vec<f32> = (0..cells)
        .map(|c| 0.025 + 0.975 * (((c * 13 + 5) % 17) as f32 / 16.0))
        .collect();
    let gy: Vec<f32> = (0..cells)
        .map(|c| 0.025 + 0.975 * (((c * 7 + 11) % 19) as f32 / 18.0))
        .collect();
    let occ: Vec<u32> = (0..cells).map(|c| if c % 5 == 0 { 4 } else { 0 }).collect();

    for with_barrier in [false, true] {
        let mut edges = FluxEdges::new(&g, &limits).expect("edges");
        let barrier_arg = with_barrier.then_some((&barrier, occ.as_slice()));
        vascular
            .fill_edges(&plan, &gx, &gy, barrier_arg, &mut edges)
            .expect("fill_edges");

        let mut pair = FieldPair::zeroed(g, &limits).expect("pair");
        pair.enable_vascular(VASCULAR_INITIAL_CONDUCTANCE, &limits)
            .expect("enable_vascular");
        pair.current_mut()
            .set_vascular_edges(&gx, &gy)
            .expect("set edges");

        let mut left = Cell::EMPTY;
        left.energy = 10.0;
        left.moments[0] = 10.0;
        let mut right = Cell::EMPTY;
        right.energy = 10.0;
        right.moments[1] = 10.0;
        for y in 0..32_u32 {
            for x in 0..32_u32 {
                let cell = if x < 16 { &left } else { &right };
                pair.current_mut().set_cell(x, y, cell).expect("set cell");
            }
        }
        let energy_initial = pair.current().energy_total();
        let mut scratch = TransportScratch::new(&g, &limits).expect("scratch");
        for _ in 0..100 {
            let (cur, next) = pair.stage();
            transport_flux(&plan, &edges, cur, next, &mut scratch).expect("flux");
            pair.commit();
        }
        let energy_final = pair.current().energy_total();
        let rel_drift = (energy_final - energy_initial).abs() / energy_initial;
        assert!(
            rel_drift < 1.0e-6,
            "with_barrier={with_barrier}: conservation drift {rel_drift:e} exceeds 1e-6"
        );
        pair.current()
            .check_invariants(
                ConeTolerance::new(f64::from(f32::MIN_POSITIVE), 1.0e-6).expect("tol"),
            )
            .expect("INV-01 and INV-02 hold");
    }

    // C.8 cancel-safety: staging a vascular Phase 2 step without calling
    // `pair.commit()` leaves committed `edge_x_cond` and `edge_y_cond` bit-exact.
    let mut pair = FieldPair::zeroed(g, &limits).expect("pair");
    pair.enable_vascular(VASCULAR_INITIAL_CONDUCTANCE, &limits)
        .expect("enable");
    pair.current_mut().energy_mut()[0] = 50.0;
    let committed_before_x: Vec<u32> = pair
        .current()
        .vascular_edges()
        .expect("edges")
        .0
        .iter()
        .map(|v| v.to_bits())
        .collect();
    let committed_before_y: Vec<u32> = pair
        .current()
        .vascular_edges()
        .expect("edges")
        .1
        .iter()
        .map(|v| v.to_bits())
        .collect();
    let mut work = FieldWork::new(&g, &limits, NonZeroUsize::MIN).expect("work");
    let (prev_x, prev_y) = pair.current().vascular_edges().expect("edges");
    let edges = work
        .prepare_vascular_edges(&vascular, &plan, prev_x, prev_y, None)
        .expect("prep")
        .clone();
    let kernel = SourceKernel::new(2.0, 3.0).expect("kernel");
    advance_field_wave2_on(
        &mut pair,
        &plan,
        &kernel,
        &[],
        step,
        FieldStepPhase2 {
            source_kernels: None,
            carrier0_edges: Some(&edges),
            vascular: Some(&vascular),
            trail_plan: None,
            trail_edges: None,
        },
        &mut work,
    )
    .expect("stage");
    // Candidate updated, committed untouched until commit()!
    let candidate_x = pair.next().vascular_edges().expect("next").0;
    assert_ne!(
        candidate_x[0].to_bits(),
        committed_before_x[0],
        "candidate vascular edge must update on active flux"
    );
    let committed_after_x: Vec<u32> = pair
        .current()
        .vascular_edges()
        .expect("edges")
        .0
        .iter()
        .map(|v| v.to_bits())
        .collect();
    let committed_after_y: Vec<u32> = pair
        .current()
        .vascular_edges()
        .expect("edges")
        .1
        .iter()
        .map(|v| v.to_bits())
        .collect();
    assert_eq!(committed_after_x, committed_before_x, "C.8 cancel-safe x");
    assert_eq!(committed_after_y, committed_before_y, "C.8 cancel-safe y");
}

#[test]
fn vascular_murray_branching_vs_sublinear_control() {
    let g = grid(32, 32, 32.0, 32.0);
    let step = TimeStep::new(1, 32).expect("step");
    let limits = BufferLimits::host();
    let kernel = SourceKernel::new(2.0, 3.0).expect("kernel");
    let src_idx = 16 * 32 + 6;
    let sinks = [10 * 32 + 24, 22 * 32 + 24];

    let run = |mu: f64| -> (f64, f64, f64) {
        // `flux_half_saturation` is `EnergyPerSecond` (`0.05 energy/tick * 32 ticks/s = 1.6`).
        let vascular = VascularField::new(0.025, 0.32, 3.0, 0.6, 1.6, mu).expect("vascular");
        let plan = vascular.transport_plan(&g, step, 0.0).expect("plan");
        let mut pair = FieldPair::zeroed(g, &limits).expect("pair");
        pair.enable_vascular(vascular.initial_conductance(), &limits)
            .expect("enable");
        let mut work = FieldWork::new(&g, &limits, NonZeroUsize::MIN).expect("work");
        work.reserve_flux().expect("reserve_flux");
        let mut edges = FluxEdges::new(&g, &limits).expect("edges");

        for _ in 0..500 {
            let energy = pair.current_mut().energy_mut();
            energy[src_idx] += 4.0;
            for &sink in &sinks {
                energy[sink] *= 0.25;
            }
            let (gx, gy) = pair.current().vascular_edges().expect("edges");
            vascular
                .fill_edges(&plan, gx, gy, None, &mut edges)
                .expect("fill");
            advance_field_wave2_on(
                &mut pair,
                &plan,
                &kernel,
                &[],
                step,
                FieldStepPhase2 {
                    source_kernels: None,
                    carrier0_edges: Some(&edges),
                    vascular: Some(&vascular),
                    trail_plan: None,
                    trail_edges: None,
                },
                &mut work,
            )
            .expect("step");
            pair.commit();
        }
        let (gx, gy) = pair.current().vascular_edges().expect("final edges");
        let total = (gx.len() + gy.len()) as f64;
        let high = gx.iter().chain(gy).filter(|&&v| v >= 0.6).count() as f64 / total;
        let mid = gx.iter().chain(gy).filter(|&&v| v > 0.1 && v < 0.6).count() as f64 / total;
        let low = gx.iter().chain(gy).filter(|&&v| v <= 0.1).count() as f64 / total;
        (high, mid, low)
    };

    let (high_15, mid_15, low_15) = run(1.5);
    let (high_07, mid_07, _low_07) = run(0.7);
    assert!(
        (0.02..=0.25).contains(&high_15),
        "μ=1.5 must carve 2-25% high-conductance vessels, got {:.2}%",
        high_15 * 100.0
    );
    assert!(
        low_15 >= 0.70,
        "μ=1.5 must prune unused background edges, got {:.2}% low",
        low_15 * 100.0
    );
    let sharpness_15 = high_15 / mid_15.max(1.0e-9);
    let sharpness_07 = high_07 / mid_07.max(1.0e-9);
    assert!(
        sharpness_15 >= 4.0 && sharpness_15 > 3.0 * sharpness_07,
        "μ=1.5 must carve a sharp bimodal channel vs diffuse μ=0.7 control: \
         μ=1.5 (high={:.2}%, mid={:.2}%, sharp={sharpness_15:.2}) vs \
         μ=0.7 (high={:.2}%, mid={:.2}%, sharp={sharpness_07:.2})",
        high_15 * 100.0,
        mid_15 * 100.0,
        high_07 * 100.0,
        mid_07 * 100.0,
    );
}

#[test]
fn vascular_extreme_flux_overflow_guard_and_substep_invariance() {
    let dt = 1.0 / 32.0;
    for mu in [1.5_f64, 0.7, 5.0] {
        let v = VascularField::new(0.025, 0.30, 2.0, 0.5, 1.0e-12, mu).expect("vascular");
        for extreme in [1.0e35_f64, 1.0e200, f64::INFINITY] {
            let g_next = v.update_conductance(0.25, extreme, dt);
            assert!(
                g_next.is_finite() && (0.025..=1.0).contains(&g_next),
                "mu={mu} flux={extreme:e} produced invalid {g_next:?}"
            );
            // At sat = 1.0, exact formula: 0.25 + dt * (2.0 * 0.75 - 0.5 * 0.225).
            let expected = (0.25_f64 + dt * (2.0 * (1.0 - 0.25) - 0.5 * (0.25 - 0.025))) as f32;
            assert_eq!(g_next.to_bits(), expected.to_bits(), "mu={mu} sat==1 exact");
        }
        for zero_like in [0.0_f64, -1.0, f64::NEG_INFINITY, f64::NAN] {
            let g_zero = v.update_conductance(0.25, zero_like, dt);
            let expected_zero = (0.25_f64 + dt * (-0.5 * (0.25 - 0.025))) as f32;
            assert_eq!(
                g_zero.to_bits(),
                expected_zero.to_bits(),
                "mu={mu} non-positive/NaN flux {zero_like:?} must yield sat=0"
            );
        }
    }

    // Substep invariance: 1 substep with r_x = 0.20 vs 2 substeps with r_x = 0.10
    // share identical total diffusive conductance `edge * substeps`, yielding
    // bit-exact updated conductances!
    let step = TimeStep::new(1, 32).expect("step");
    let limits = BufferLimits::host();
    let g1 = grid(8, 8, 8.0, 8.0); // h = 1.0
    let g2 = grid(8, 8, 4.0, 4.0); // h = 0.5 -> 4x larger r -> more substeps
    let plan1 = TransportPlan::new(&g1, step, 6.4, 0.0).expect("plan1"); // r = 0.20, substeps = 1
    let plan2 = TransportPlan::new(&g2, step, 1.6, 0.0).expect("plan2"); // same r_total = 0.20!
    // Force a true multi-substep plan at 2x diffusion on g1 (r_total = 0.40 -> substeps = 2, r_sub = 0.20)
    // vs half-g with 2x plan: compare edge*substeps invariant directly!
    let plan_sub2 = TransportPlan::new(&g1, step, 12.8, 0.0).expect("sub2");
    assert_eq!(plan1.substeps(), 1);
    assert_eq!(plan2.substeps(), 1);
    assert_eq!(plan_sub2.substeps(), 2);
    assert_eq!(plan1.ratios().0, plan_sub2.ratios().0);

    let v = VascularField::new(0.025, 6.4, 2.0, 0.5, 0.1, 1.5).expect("v");
    let cells = g1.cell_count() as usize;
    let energy: Vec<f32> = (0..cells).map(|c| (c % 7) as f32 * 2.5).collect();
    let prev_x = vec![0.4_f32; cells];
    let prev_y = vec![0.4_f32; cells];
    let mut edges_1 = FluxEdges::new(&g1, &limits).expect("e1");
    let mut edges_2 = FluxEdges::new(&g1, &limits).expect("e2");
    // Same total edge conductance across the tick: edges_1 = 0.10 (1 substep),
    // edges_2 = 0.05 (2 substeps) => edges_1 * 1 == edges_2 * 2!
    edges_1.edge_x_mut().fill(0.10);
    edges_1.edge_y_mut().fill(0.10);
    edges_2.edge_x_mut().fill(0.05);
    edges_2.edge_y_mut().fill(0.05);
    let mut nx1 = vec![0.0_f32; cells];
    let mut ny1 = vec![0.0_f32; cells];
    let mut nx2 = vec![0.0_f32; cells];
    let mut ny2 = vec![0.0_f32; cells];
    v.update_edges(
        &plan1,
        step,
        &edges_1,
        &energy,
        (&prev_x, &prev_y),
        (&mut nx1, &mut ny1),
    )
    .expect("u1");
    v.update_edges(
        &plan_sub2,
        step,
        &edges_2,
        &energy,
        (&prev_x, &prev_y),
        (&mut nx2, &mut ny2),
    )
    .expect("u2");
    let b1x: Vec<u32> = nx1.iter().map(|v| v.to_bits()).collect();
    let b2x: Vec<u32> = nx2.iter().map(|v| v.to_bits()).collect();
    let b1y: Vec<u32> = ny1.iter().map(|v| v.to_bits()).collect();
    let b2y: Vec<u32> = ny2.iter().map(|v| v.to_bits()).collect();
    assert_eq!(b1x, b2x, "substep-invariant east update must be bit-exact");
    assert_eq!(b1y, b2y, "substep-invariant north update must be bit-exact");
}

/// `VascularField::update_edges` divides per-tick diffusive transfer by `dt` so
/// `flux_x = (edges.edge_x[c] * (substeps / dt)) * |e_c - e_e|` has true
/// `EnergyPerSecond` units matching `v2.vascular.flux_half_saturation`:
/// instantaneous flux rate and ODE evolution are invariant under halving `dt`
/// (including across substep-count transitions).
#[test]
fn vascular_flux_dt_invariance_energy_per_second() {
    let g = grid(8, 8, 8.0, 8.0);
    let limits = BufferLimits::host();
    let cells = g.cell_count() as usize;
    // D_max = 12.8 forces substeps=2 at dt=1/32 (r_total=0.40) and substeps=1 at
    // dt=1/64 (0.20) and dt=1/128 (0.10), testing both dt and substep invariance!
    let vascular = VascularField::new(0.025, 12.8, 2.0, 0.5, 1.5, 1.5).expect("vascular");
    let energy: Vec<f32> = (0..cells).map(|c| ((c % 7) as f32) * 2.5).collect();
    let prev_x = vec![0.25_f32; cells];
    let prev_y = vec![0.25_f32; cells];

    let steps = [
        (TimeStep::new(1, 32).expect("1/32"), 16_usize, 2_u32),
        (TimeStep::new(1, 64).expect("1/64"), 32_usize, 1_u32),
        (TimeStep::new(1, 128).expect("1/128"), 64_usize, 1_u32),
    ];

    // 1. Instantaneous rate `(g_next - g_prev) / dt` matches across dt = 1/32, 1/64, 1/128.
    let mut rates_x: Vec<Vec<f64>> = Vec::new();
    let mut rates_y: Vec<Vec<f64>> = Vec::new();
    for &(step, _, expected_substeps) in &steps {
        let plan = vascular.transport_plan(&g, step, 0.0).expect("plan");
        assert_eq!(plan.substeps(), expected_substeps);
        let mut edges = FluxEdges::new(&g, &limits).expect("edges");
        vascular
            .fill_edges(&plan, &prev_x, &prev_y, None, &mut edges)
            .expect("fill");
        let dt = f64::from(step.numerator()) / f64::from(step.denominator());
        let rate_scale = f64::from(plan.substeps()) / dt;
        // `edges.edge_x[c] * (substeps / dt) == D_max * g / h^2 == 12.8 * 0.25 == 3.2` for all dt!
        for &ex in edges.edge_x() {
            assert!(
                (ex * rate_scale - 3.2).abs() <= 1.0e-12,
                "dt={dt}: flux rate coefficient {} != 3.2",
                ex * rate_scale
            );
        }
        let mut nx = vec![0.0_f32; cells];
        let mut ny = vec![0.0_f32; cells];
        vascular
            .update_edges(
                &plan,
                step,
                &edges,
                &energy,
                (&prev_x, &prev_y),
                (&mut nx, &mut ny),
            )
            .expect("update");
        rates_x.push(
            nx.iter()
                .zip(&prev_x)
                .map(|(&n, &p)| (f64::from(n) - f64::from(p)) / dt)
                .collect(),
        );
        rates_y.push(
            ny.iter()
                .zip(&prev_y)
                .map(|(&n, &p)| (f64::from(n) - f64::from(p)) / dt)
                .collect(),
        );
    }
    for idx in 1..rates_x.len() {
        for c in 0..cells {
            assert!(
                (rates_x[idx][c] - rates_x[0][c]).abs() <= 1.0e-4,
                "cell {c} east dg/dt diverged across dt: {} vs {}",
                rates_x[idx][c],
                rates_x[0][c]
            );
            assert!(
                (rates_y[idx][c] - rates_y[0][c]).abs() <= 1.0e-4,
                "cell {c} north dg/dt diverged across dt: {} vs {}",
                rates_y[idx][c],
                rates_y[0][c]
            );
        }
    }

    // 2. Integrating T = 0.5 s at dt = 1/32, 1/64, 1/128 converges to the same
    //    conductance profile (within explicit-Euler O(dt) error).
    let mut finals = Vec::new();
    for &(step, ticks, _) in &steps {
        let plan = vascular.transport_plan(&g, step, 0.0).expect("plan");
        let mut edges = FluxEdges::new(&g, &limits).expect("edges");
        let mut gx = prev_x.clone();
        let mut gy = prev_y.clone();
        let mut nx = vec![0.0_f32; cells];
        let mut ny = vec![0.0_f32; cells];
        for _ in 0..ticks {
            vascular
                .fill_edges(&plan, &gx, &gy, None, &mut edges)
                .expect("fill");
            vascular
                .update_edges(&plan, step, &edges, &energy, (&gx, &gy), (&mut nx, &mut ny))
                .expect("update");
            std::mem::swap(&mut gx, &mut nx);
            std::mem::swap(&mut gy, &mut ny);
        }
        finals.push((gx, gy));
    }
    let max_diff_64_128 = finals[1]
        .0
        .iter()
        .chain(&finals[1].1)
        .zip(finals[2].0.iter().chain(&finals[2].1))
        .map(|(&a, &b)| (f64::from(a) - f64::from(b)).abs())
        .fold(0.0_f64, f64::max);
    let max_diff_32_128 = finals[0]
        .0
        .iter()
        .chain(&finals[0].1)
        .zip(finals[2].0.iter().chain(&finals[2].1))
        .map(|(&a, &b)| (f64::from(a) - f64::from(b)).abs())
        .fold(0.0_f64, f64::max);
    assert!(
        max_diff_64_128 < 0.01 && max_diff_64_128 < max_diff_32_128,
        "O(dt) convergence failed: diff(1/64,1/128)={max_diff_64_128:.5}, diff(1/32,1/128)={max_diff_32_128:.5}"
    );
}

#[test]
fn vascular_disabled_zero_alloc_and_bit_exact() {
    let limits = BufferLimits::host();
    let v1 = ValidatedConfig::from_toml(&appendix_a_toml()).expect("v1");
    assert_eq!(VascularField::from_config(&v1).expect("v1"), None);
    let (v1_state, _) = FieldState::initial(&v1, &limits).expect("v1 init");
    assert!(!v1_state.has_vascular());
    assert!(v1_state.vascular_edges().is_none());

    let disabled_toml = v2_toml(
        "[v2]\nprofile = \"supercolony\"\n[v2.vascular]\nenabled = false\n[v2.carriers]\nenabled = false",
    );
    let disabled = ValidatedConfig::from_toml(&disabled_toml).expect("disabled");
    assert_eq!(
        VascularField::from_config(&disabled).expect("disabled"),
        None
    );
    let (dis_state, _) = FieldState::initial(&disabled, &limits).expect("dis init");
    assert!(!dis_state.has_vascular());
    assert_eq!(dis_state.allocated_bytes(), v1_state.allocated_bytes());

    let enabled_toml = v2_toml(
        "[v2]\nprofile = \"supercolony\"\n[v2.vascular]\nenabled = true\n[v2.carriers]\nenabled = false",
    );
    let enabled = ValidatedConfig::from_toml(&enabled_toml).expect("enabled");
    assert!(
        VascularField::from_config(&enabled)
            .expect("enabled")
            .is_some()
    );
    let (en_state, _) = FieldState::initial(&enabled, &limits).expect("en init");
    assert!(en_state.has_vascular());
    let expected_extra = FieldFootprint::vascular_edge_bytes(en_state.grid()).expect("bytes");
    assert_eq!(
        en_state.allocated_bytes() - dis_state.allocated_bytes(),
        expected_extra
    );

    // Bit-exact baseline parity through advance_field_wave2_on.
    let g = grid(16, 16, 16.0, 16.0);
    let step = TimeStep::new(1, 32).expect("step");
    let plan = TransportPlan::new(&g, step, 0.2, 0.01).expect("plan");
    let kernel = SourceKernel::new(2.0, 3.0).expect("kernel");
    let sources = [Source::new(SourceId::new(0), [8.0, 8.0], unit_channel(0), 12.0).expect("s")];
    let two = NonZeroUsize::new(2).expect("2");
    let mut p1 = FieldPair::zeroed(g, &limits).expect("p1");
    let mut p2 = FieldPair::zeroed(g, &limits).expect("p2");
    let mut w1 = FieldWork::new(&g, &limits, two).expect("w1");
    let mut w2 = FieldWork::new(&g, &limits, two).expect("w2");
    let r1 = advance_field_on(&mut p1, &plan, &kernel, &sources, step, &mut w1).expect("base");
    let r2 = advance_field_wave2_on(
        &mut p2,
        &plan,
        &kernel,
        &sources,
        step,
        FieldStepPhase2::BASELINE,
        &mut w2,
    )
    .expect("wave2");
    assert_eq!(r1, r2);
    let b1: Vec<u32> = p1.next().as_slice().iter().map(|v| v.to_bits()).collect();
    let b2: Vec<u32> = p2.next().as_slice().iter().map(|v| v.to_bits()).collect();
    assert_eq!(b1, b2);
}

// ------------------------------------------------- V2-15 dual-carrier-trails

#[test]
fn dual_carrier_independent_cones_and_exact_ledger() {
    let g = grid(32, 32, 32.0, 32.0);
    let step = TimeStep::new(1, 32).expect("step");
    let limits = BufferLimits::host();
    let d0 = 0.8;
    let l0 = 0.20;
    let rules = DualCarrierRules::new(d0 / 100.0, l0 / 20.0, 0.20, 1.0).expect("rules");
    let plan0 = TransportPlan::new(&g, step, d0, l0).expect("plan0");
    let plan1 = rules.trail_plan(&g, step).expect("plan1");
    assert_eq!(plan1.substeps(), 1);

    let mut pair_1w = FieldPair::zeroed_with_planes(g, DUAL_CARRIER_PLANES, &limits).expect("p1");
    let mut pair_4w = FieldPair::zeroed_with_planes(g, DUAL_CARRIER_PLANES, &limits).expect("p4");
    assert_eq!(pair_1w.current().planes(), 34);
    assert!(pair_1w.current().is_dual_carrier());
    assert_eq!(pair_1w.current().carrier0(), CarrierLayout::CARRIER_0);
    assert_eq!(pair_1w.current().carrier1(), Some(CarrierLayout::CARRIER_1));

    // Seed identical sharp spikes on Carrier 0 (e_0 = 20, m^(0)_0 = 20) and
    // Carrier 1 (e_1 = 20, m^(1)_0 = 20) at cell (16, 16).
    let mut spike0 = Cell::EMPTY;
    spike0.energy = 20.0;
    spike0.moments[0] = 20.0;
    let mut spike1_m = [0.0_f32; 8];
    spike1_m[0] = 20.0;
    for pair in [&mut pair_1w, &mut pair_4w] {
        pair.current_mut().set_cell(16, 16, &spike0).expect("c0");
        pair.current_mut()
            .set_trail_cell(16, 16, 20.0, &spike1_m)
            .expect("c1");
    }

    // Verify DeclaredLayout & FrankenNumPy ArrayStorage & relayout round-trip on 34 planes.
    let declared = pair_1w.current().layout().describe();
    let admitted = FieldLayout::from_declared(&declared).expect("admit 34-plane declared");
    assert_eq!(admitted.planes(), 34);
    let interleaved =
        FieldLayout::new_with_planes(g, Arrangement::Interleaved, 34).expect("interleaved 34");
    let moved = relayout(&pair_1w.current().view(), &interleaved).expect("relayout 34");
    assert_eq!(moved.len(), pair_1w.current().as_slice().len());
    let (storage, layout) = pair_1w.current().clone().into_array_storage();
    let roundtrip = FieldState::from_array_storage(storage, &layout).expect("roundtrip 34");
    assert_eq!(roundtrip, *pair_1w.current());

    let kernel = SourceKernel::new(2.0, 3.0).expect("kernel");
    let mut w1 = FieldWork::new(&g, &limits, NonZeroUsize::MIN).expect("w1");
    let mut w4 = FieldWork::new(&g, &limits, NonZeroUsize::new(4).expect("4")).expect("w4");
    let tol = ConeTolerance::new(f64::from(f32::MIN_POSITIVE), 1.0e-6).expect("tol");

    let initial_total = pair_1w.current().energy_total();
    assert_eq!(initial_total, 40.0);
    let mut total_decay = 0.0_f64;

    for _ in 0..128 {
        let phase2 = FieldStepPhase2 {
            source_kernels: None,
            carrier0_edges: None,
            vascular: None,
            trail_plan: Some(&plan1),
            trail_edges: None,
        };
        let r1 = advance_field_wave2_on(&mut pair_1w, &plan0, &kernel, &[], step, phase2, &mut w1)
            .expect("1w");
        let r4 = advance_field_wave2_on(&mut pair_4w, &plan0, &kernel, &[], step, phase2, &mut w4)
            .expect("4w");
        assert_eq!(r1, r4);
        assert!(
            r1.residual.abs() < 1.0e-5,
            "dual-carrier tick residual {:e} exceeded 1e-5",
            r1.residual
        );
        total_decay += r1.transport.decay_removed;
        pair_1w.commit();
        pair_4w.commit();
    }
    let b1: Vec<u32> = pair_1w
        .current()
        .as_slice()
        .iter()
        .map(|v| v.to_bits())
        .collect();
    let b4: Vec<u32> = pair_4w
        .current()
        .as_slice()
        .iter()
        .map(|v| v.to_bits())
        .collect();
    assert_eq!(b1, b4, "34-plane 1 vs 4 workers must be bit-exact");

    // Global ledger closure across BOTH carriers.
    let final_total = pair_1w.current().energy_total();
    let ledger_err = ((final_total - initial_total) + total_decay).abs();
    assert!(
        ledger_err < 1.0e-5 * initial_total,
        "128-tick dual-carrier ledger drift {ledger_err:e}"
    );

    // Carrier 1 must stay >= 5x sharper at the spike center than Carrier 0.
    let peak0 = f64::from(pair_1w.current().cell(16, 16).expect("c0").energy);
    let peak1 = f64::from(pair_1w.current().trail_cell(16, 16).expect("c1").0);
    assert!(
        peak1 >= 5.0 * peak0,
        "Carrier 1 peak {peak1:.4} must stay >= 5x sharper than Carrier 0 peak {peak0:.4}"
    );

    // Dual-cone invariants: check_invariants_on (1 vs 4 workers), scan_invariants,
    // and summarize_scan must all agree bit-for-bit!
    let s1 = pair_1w
        .current()
        .check_invariants_on(tol, 1)
        .expect("1w inv");
    let s4 = pair_1w
        .current()
        .check_invariants_on(tol, 4)
        .expect("4w inv");
    assert_eq!(s1, s4);
    let scan = pair_1w.current().scan_invariants(tol);
    let summarized = pair_1w
        .current()
        .summarize_scan(tol, &scan)
        .expect("summarize")
        .expect("no offense");
    assert_eq!(summarized, s1);

    // Plant a Carrier-1 cone violation (`m^(1)_0 = 2.0 > e_1 = 0.5`) and verify
    // BOTH `check_invariants` and `scan_invariants` catch it!
    let mut bad = pair_1w.current().clone();
    let mut bad_m1 = [0.0_f32; 8];
    bad_m1[0] = 2.0;
    bad.set_trail_cell(3, 4, 0.5, &bad_m1).expect("plant");
    assert!(matches!(
        bad.check_invariants(tol),
        Err(CoreError::NumericalInvalid { .. })
    ));
    let bad_scan = bad.scan_invariants(tol);
    assert_eq!(bad_scan.offending, Some(4 * 32 + 3));
}

#[test]
fn dual_carrier_grants_debit_leaves_carrier1_bit_identical() {
    let g = grid(16, 16, 16.0, 16.0);
    let step = TimeStep::new(1, 32).expect("step");
    let limits = BufferLimits::host();

    let mut field_1w = FieldState::zeroed_dual_carrier(g, &limits).expect("f1");
    let mut c0 = Cell::EMPTY;
    c0.energy = 5.0;
    c0.moments[0] = 4.0;
    c0.moments[23] = -1.0;
    field_1w.set_cell(8, 8, &c0).expect("set c0");
    let c1_m = [1.5_f32, -0.25, 0.1, 0.0, 0.3, -0.4, 0.2, -0.5];
    field_1w.set_trail_cell(8, 8, 2.0, &c1_m).expect("set c1");
    let mut field_4w = field_1w.clone();

    let c1_before: Vec<u32> = (CARRIER1_ENERGY_PLANE..DUAL_CARRIER_PLANES)
        .flat_map(|p| field_1w.plane(p).expect("p").iter().map(|v| v.to_bits()))
        .collect();

    // 50 hungry organisms feeding at (8.5, 8.5) on Carrier 0.
    let demands: Vec<Demand> = (1_u64..=50)
        .map(|id| Demand {
            organism: OrganismId::new(id),
            position: [8.5, 8.5],
            amount: 0.5,
        })
        .collect();
    let mut grants_1w = Grants::with_capacity(&g, 50, &limits).expect("g1");
    let mut grants_4w = Grants::with_capacity(&g, 50, &limits).expect("g4");
    resolve_uptake(&field_1w, &demands, &mut grants_1w).expect("res1");
    resolve_uptake(&field_4w, &demands, &mut grants_4w).expect("res4");

    let mut e0_1 = Deposits::with_capacity(&g, 4, &limits).expect("d");
    let mut r0_1 = Deposits::with_capacity(&g, 4, &limits).expect("d");
    let mut t1_1 = Deposits::with_capacity(&g, 4, &limits).expect("d");
    let mut e0_4 = Deposits::with_capacity(&g, 4, &limits).expect("d");
    let mut r0_4 = Deposits::with_capacity(&g, 4, &limits).expect("d");
    let mut t1_4 = Deposits::with_capacity(&g, 4, &limits).expect("d");

    reduce_dual_carrier_field_on(
        &mut field_1w,
        &mut grants_1w,
        [&mut e0_1, &mut r0_1],
        &mut t1_1,
        1,
    )
    .expect("red 1w");
    reduce_dual_carrier_field_on(
        &mut field_4w,
        &mut grants_4w,
        [&mut e0_4, &mut r0_4],
        &mut t1_4,
        4,
    )
    .expect("red 4w");

    // Carrier 0 drained, 1w vs 4w bit-exact.
    assert_eq!(field_1w.cell(8, 8).expect("c0").energy, 0.0);
    let all_1: Vec<u32> = field_1w.as_slice().iter().map(|v| v.to_bits()).collect();
    let all_4: Vec<u32> = field_4w.as_slice().iter().map(|v| v.to_bits()).collect();
    assert_eq!(all_1, all_4);

    // Carrier 1 (plane 25 e_1 AND planes 26..=33 m^(1)) BIT-FOR-BIT UNCHANGED!
    let c1_after: Vec<u32> = (CARRIER1_ENERGY_PLANE..DUAL_CARRIER_PLANES)
        .flat_map(|p| field_1w.plane(p).expect("p").iter().map(|v| v.to_bits()))
        .collect();
    assert_eq!(
        c1_after, c1_before,
        "Carrier 1 trails must remain bit-for-bit untouched during Carrier 0 uptake"
    );

    // External Gaussian sources must ALSO leave Carrier 1 bit-for-bit untouched!
    let mut pair = FieldPair::from_states(field_1w.clone(), field_1w).expect("pair");
    let plan0 = TransportPlan::new(&g, step, 0.0, 0.0).expect("plan0");
    let plan1 = TransportPlan::new(&g, step, 0.0, 0.0).expect("plan1");
    let mut src_dir = [0.0; CHANNELS];
    src_dir[0] = 0.6;
    src_dir[31] = 0.8; // Carrier-1 tail channel in source direction MUST NOT leak into Carrier 1!
    let sources = [Source::new(SourceId::new(0), [8.5, 8.5], src_dir, 32.0).expect("src")];
    let mut work = FieldWork::new(&g, &limits, NonZeroUsize::MIN).expect("work");
    for fused in [false, true] {
        let kernel = if fused {
            SourceKernel::new_fused(2.0, 3.0).expect("fused")
        } else {
            SourceKernel::new(2.0, 3.0).expect("seq")
        };
        advance_field_wave2_on(
            &mut pair,
            &plan0,
            &kernel,
            &sources,
            step,
            FieldStepPhase2 {
                source_kernels: None,
                carrier0_edges: None,
                vascular: None,
                trail_plan: Some(&plan1),
                trail_edges: None,
            },
            &mut work,
        )
        .expect("src step");
        let c1_post_src: Vec<u32> = (CARRIER1_ENERGY_PLANE..DUAL_CARRIER_PLANES)
            .flat_map(|p| pair.next().plane(p).expect("p").iter().map(|v| v.to_bits()))
            .collect();
        assert_eq!(
            c1_post_src, c1_before,
            "fused={fused}: external sources must never write into Carrier 1"
        );
    }
}

#[test]
fn dual_carrier_sub_unit_split_zero_nan_and_transfer_bounds() {
    let g = grid(16, 16, 16.0, 16.0);
    let limits = BufferLimits::host();

    let mut d_only_c0 = [0.0; CHANNELS];
    d_only_c0[0] = 1.0; // d^(1) == 0
    let mut d_only_c1 = [0.0; CHANNELS];
    d_only_c1[24] = 1.0; // d^(0) == 0
    let mut d_mixed = [0.0; CHANNELS];
    d_mixed[3] = 0.6;
    d_mixed[27] = -0.8; // ||d|| = 1.0

    for (label, dir, paid, eff, share) in [
        ("d1_zero", &d_only_c0, 3.0, 1.0, 0.5),
        ("d0_zero", &d_only_c1, 3.0, 1.0, 0.5),
        ("mixed_p05", &d_mixed, 5.25, 0.85, 0.5),
        ("tiny_paid", &d_mixed, 1.0e-30, 1.0, 0.2),
        ("zero_paid", &d_mixed, 0.0, 1.0, 0.2),
    ] {
        let split = split_dual_carrier_emission(paid, eff, share, dir).expect(label);
        assert!(split.paid_0.is_finite() && split.paid_0 >= 0.0);
        assert!(split.paid_1.is_finite() && split.paid_1 >= 0.0);
        assert!(split.paid_0 + split.paid_1 <= paid + 1.0e-15 * paid);
        assert!(split.offered_0 <= split.paid_0);
        assert!(split.offered_1 <= split.paid_1);
        assert!(split.direction_0.iter().all(|v| v.is_finite()));
        assert!(split.direction_1.iter().all(|v| v.is_finite()));
        // Carrier-1 direction packed into slots 0..8, trailing 8..32 zero.
        assert!(split.direction_1[8..].iter().all(|&v| v == 0.0));
        assert!(split.direction_0[24..].iter().all(|&v| v == 0.0));

        // Reduce into a 34-plane field and verify applied_0 + applied_1 <= paid & cones hold!
        let mut field = FieldState::zeroed_dual_carrier(g, &limits).expect("field");
        let mut d0 = Deposits::with_capacity(&g, 1, &limits).expect("d0");
        let mut d1 = Deposits::with_capacity(&g, 1, &limits).expect("d1");
        d0.push(
            &g,
            OrganismId::new(1),
            [8.25, 8.75],
            split.paid_0,
            split.offered_0,
            &split.direction_0,
        )
        .expect("push 0");
        d1.push(
            &g,
            OrganismId::new(1),
            [8.25, 8.75],
            split.paid_1,
            split.offered_1,
            &split.direction_1,
        )
        .expect("push 1");
        let s0 = apply_deposits_to_carrier_on(&mut field, &mut d0, CarrierLayout::CARRIER_0, 1)
            .expect("apply 0");
        let s1 = apply_deposits_to_carrier_on(&mut field, &mut d1, CarrierLayout::CARRIER_1, 1)
            .expect("apply 1");
        assert!(
            s0.applied + s1.applied <= paid,
            "{label}: applied_0 ({}) + applied_1 ({}) exceeded paid ({paid})",
            s0.applied,
            s1.applied
        );
        field
            .check_invariants(
                ConeTolerance::new(f64::from(f32::MIN_POSITIVE), 1.0e-6).expect("tol"),
            )
            .expect("dual cones hold after split deposit");
        if label == "mixed_p05" {
            // Confirm channel 27 (-0.8) landed in Carrier 1 moment 3 (plane 26 + 3 = 29).
            let plane29_sum: f64 = field
                .plane(CARRIER1_FIRST_MOMENT_PLANE + 3)
                .expect("p29")
                .iter()
                .map(|&v| f64::from(v))
                .sum();
            assert!(
                plane29_sum < -0.5,
                "expected negative m^(1)_3 trail deposit"
            );
        }
    }
}

#[derive(Debug, Default)]
struct V1OnlyFieldAndSensor {
    advance_calls: usize,
}

impl FieldBackend for V1OnlyFieldAndSensor {
    fn describe(&self) -> String {
        "v1-only".into()
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

impl SensorBackend for V1OnlyFieldAndSensor {
    fn describe(&self) -> String {
        "v1-only-sensor".into()
    }
    fn sample(
        &mut self,
        _: &SensorPyramid,
        _: &FieldState,
        _: &[SensorQuery],
        samples: &mut Vec<RawSample>,
    ) -> Result<(), CoreError> {
        samples.clear();
        Ok(())
    }
}

#[test]
fn dual_carrier_disabled_matches_33_plane_bit_exact_and_full_wave2_composition() {
    let limits = BufferLimits::host();
    let v1 = ValidatedConfig::from_toml(&appendix_a_toml()).expect("v1");
    assert_eq!(DualCarrierRules::from_config(&v1).expect("v1"), None);
    assert_eq!(field_planes_for_config(&v1), FIELD_PLANES);

    let disabled_toml = v2_toml(
        "[v2]\nprofile = \"supercolony\"\n[v2.carriers]\nenabled = false\n[v2.vascular]\nenabled = false",
    );
    let disabled = ValidatedConfig::from_toml(&disabled_toml).expect("disabled");
    assert_eq!(DualCarrierRules::from_config(&disabled).expect("dis"), None);
    assert_eq!(field_planes_for_config(&disabled), FIELD_PLANES);
    assert_eq!(
        FieldLayout::for_config(&disabled).expect("layout").planes(),
        FIELD_PLANES
    );
    let pyr_dis = SensorPyramid::from_config(&disabled, &limits).expect("pyr dis");
    assert_eq!(pyr_dis.planes(), FIELD_PLANES as usize);

    // 34-plane SensorPyramid build (1 vs 4 workers) + sample_dual + sample_dual_batch.
    let g = grid(16, 16, 16.0, 16.0);
    let mut f34 = FieldState::zeroed_dual_carrier(g, &limits).expect("f34");
    for y in 0..16_u32 {
        for x in 0..16_u32 {
            let mut cell = Cell::EMPTY;
            cell.energy = 1.0 + x as f32 * 0.25;
            cell.moments[0] = 0.5;
            f34.set_cell(x, y, &cell).expect("c0");
            let m1 = [0.25_f32, -0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.15];
            f34.set_trail_cell(x, y, 0.5 + y as f32 * 0.125, &m1)
                .expect("c1");
        }
    }
    let mut pyr1 = SensorPyramid::new_with_planes(g, 2, MAX_PLANES, &limits).expect("p1");
    let mut pyr4 = SensorPyramid::new_with_planes(g, 2, MAX_PLANES, &limits).expect("p4");
    let v1_view = pyr1.build_on(&f34, 1).expect("b1");
    let v4_view = pyr4.build_on(&f34, 4).expect("b4");
    let queries = [
        SensorQuery {
            position: [3.25, 7.5],
            scale: 1.0,
        },
        SensorQuery {
            position: [11.75, 2.1],
            scale: 2.5,
        },
    ];
    // 33-plane `sample` refuses 34-plane pyramid cleanly!
    assert!(matches!(
        v1_view.sample(queries[0].position, queries[0].scale),
        Err(CoreError::LayoutMismatch { .. })
    ));
    let s1 = v1_view
        .sample_dual(queries[0].position, queries[0].scale)
        .expect("sd1");
    let s4 = v4_view
        .sample_dual(queries[0].position, queries[0].scale)
        .expect("sd4");
    assert_eq!(s1, s4);
    assert!(s1.values[CARRIER1_ENERGY_PLANE as usize] > 0.5);
    let mut batch = vec![
        DualRawSample {
            values: [0.0; MAX_PLANES],
            dx: [0.0; MAX_PLANES],
            dy: [0.0; MAX_PLANES],
        };
        queries.len()
    ];
    v1_view
        .sample_dual_batch(&queries, &mut batch)
        .expect("batch");
    assert_eq!(batch[0], s1);
    assert_eq!(
        batch[1],
        v1_view
            .sample_dual(queries[1].position, queries[1].scale)
            .expect("q1")
    );

    // Full 4-feature composition: V2-02 (oases) + V2-09 (tight-junctions) +
    // V2-14 (vascular-field) + V2-15 (dual-carrier-trails) with 0 B steady-state alloc!
    let step = TimeStep::new(1, 32).expect("step");
    let oasis = OasisSchedule::new(16.0, 0.1, 1.5, 2.5).expect("oasis");
    let barrier = TightJunctions::new(BarrierMode::HarmonicV1, 0.04, 1.2).expect("tj");
    let vascular = VascularField::new(0.025, 0.30, 2.0, 0.5, 0.05, 1.5).expect("vasc");
    let dual = DualCarrierRules::new(0.005, 0.002, 0.2, 1.0).expect("dual");
    let plan0 = vascular.transport_plan(&g, step, 0.02).expect("p0");
    let plan1 = dual.trail_plan(&g, step).expect("p1");
    let base_kernel = SourceKernel::new_fused(2.0, 3.0).expect("fused");
    let mut sources = vec![
        Source::new(SourceId::new(0), [4.0, 4.0], unit_channel(0), 10.0).expect("s0"),
        Source::new(SourceId::new(1), [12.0, 12.0], unit_channel(1), 10.0).expect("s1"),
        Source::new(SourceId::new(2), [4.0, 12.0], unit_channel(2), 10.0).expect("s2"),
    ];
    let kernels = [
        base_kernel.with_sigma(1.5).expect("k0"),
        base_kernel.with_sigma(2.0).expect("k1"),
        base_kernel.with_sigma(2.5).expect("k2"),
    ];
    let mut pair = FieldPair::zeroed_with_planes(g, DUAL_CARRIER_PLANES, &limits).expect("pair");
    pair.enable_vascular(vascular.initial_conductance(), &limits)
        .expect("vasc");
    let mut work = FieldWork::new(&g, &limits, NonZeroUsize::new(2).expect("2")).expect("work");
    assert!(work.trail_flux_edges().is_none());
    work.reserve_supports_for_kernels(&kernels).expect("res k");
    work.reserve_flux().expect("res f");
    work.reserve_trail_flux().expect("res tf");
    let bytes_steady = work.allocated_bytes();
    let positions = [[4.5, 4.5], [4.6, 4.4], [12.2, 12.2]];
    let mut power_scratch = Vec::new();
    for tick in 0..6_u64 {
        oasis
            .apply_powers(
                flife_core::Tick::new(tick),
                step,
                10.0,
                &mut sources,
                &mut power_scratch,
            )
            .expect("powers");
        let (gx, gy) = pair.current().vascular_edges().expect("ve");
        let c0_edges = work
            .prepare_vascular_edges(&vascular, &plan0, gx, gy, Some((&barrier, &positions)))
            .expect("c0 edges")
            .clone();
        let c1_edges = work
            .prepare_trail_tight_junctions(&barrier, &plan1)
            .expect("c1 edges")
            .clone();
        advance_field_wave2_on(
            &mut pair,
            &plan0,
            &base_kernel,
            &sources,
            step,
            FieldStepPhase2 {
                source_kernels: Some(&kernels),
                carrier0_edges: Some(&c0_edges),
                vascular: Some(&vascular),
                trail_plan: Some(&plan1),
                trail_edges: Some(&c1_edges),
            },
            &mut work,
        )
        .expect("composed wave2 step");
        pair.commit();
        assert_eq!(
            work.allocated_bytes(),
            bytes_steady,
            "tick {tick}: composed Wave-2 step must allocate 0 bytes"
        );
    }
    pair.current()
        .check_invariants(ConeTolerance::new(f64::from(f32::MIN_POSITIVE), 1.0e-6).expect("tol"))
        .expect("composed dual-carrier invariants hold");

    // Wave-2 trait default refusals on V1-only backends.
    let mut backend = V1OnlyFieldAndSensor::default();
    assert!(matches!(
        backend.advance_wave2(
            &mut pair,
            &plan0,
            &base_kernel,
            &sources,
            step,
            FieldStepPhase2::BASELINE,
        ),
        Err(CoreError::MissingCapability { .. })
    ));
    let mut grants = Grants::new();
    let mut d0 = Deposits::new();
    let mut d1 = Deposits::new();
    let mut dt = Deposits::new();
    assert!(matches!(
        backend.reduce_dual_carrier(pair.current_mut(), &mut grants, [&mut d0, &mut d1], &mut dt),
        Err(CoreError::MissingCapability { .. })
    ));
    let mut dual_samples = Vec::new();
    assert!(matches!(
        backend.sample_dual(&pyr1, &f34, &queries, &mut dual_samples),
        Err(CoreError::MissingCapability { .. })
    ));
}

#[test]
fn dual_carrier_check_invariants_blocked_matches_scalar_and_catches_every_fault() {
    // 16 x 12 = 192 cells spans three 64-cell CHECK_BLOCKs cleanly, and
    // 17 x 11 = 187 cells spans two full 64-cell blocks plus a 59-cell tail.
    let tol = ConeTolerance::new(1.0e-6, 1.0e-6).expect("tol");
    let trigger = ConeTolerance::new(1.0e-7, 1.0e-7).expect("trigger");

    let build_clean = |cx: u32, cy: u32| -> FieldState {
        let grid = FieldGrid::new(cx, cy, f64::from(cx), f64::from(cy)).expect("grid");
        let mut state =
            FieldState::zeroed_dual_carrier(grid, &BufferLimits::host()).expect("dual field");
        let cells = grid.cell_count() as usize;
        for c in 0..cells {
            if c % 7 == 0 {
                continue; // empty Carrier 0 cell
            }
            let e0 = 1.0 + (c % 13) as f32 * 0.25;
            state.energy_mut()[c] = e0;
            for ch in 0..CARRIER0_MOMENT_CHANNELS {
                let sign = if (c + ch as usize).is_multiple_of(2) {
                    1.0
                } else {
                    -1.0
                };
                state.moment_mut(ch).expect("c0 m")[c] = sign * (e0 / 6.0);
            }
            let e1 = 0.5 + (c % 5) as f32 * 0.1;
            state.plane_mut(CARRIER1_ENERGY_PLANE).expect("e1")[c] = e1;
            for ch in 0..CARRIER1_MOMENT_CHANNELS {
                let plane = CARRIER1_FIRST_MOMENT_PLANE + ch;
                state.plane_mut(plane).expect("c1 m")[c] = e1 / 4.0;
            }
        }
        state
    };

    for (cx, cy) in [(16, 12), (17, 11)] {
        let state = build_clean(cx, cy);
        let s1 = state
            .check_invariants_with_trigger_on(tol, trigger, 1)
            .expect("1w clean");
        let s4 = state
            .check_invariants_with_trigger_on(tol, trigger, 4)
            .expect("4w clean");
        assert_eq!(s1, s4, "worker parity on {cx}x{cy}");
        let scan = state.scan_invariants(tol);
        assert!(scan.offending.is_none());
        let from_scan = state
            .summarize_scan(tol, &scan)
            .expect("summarize")
            .expect("passing");
        assert_eq!(s1.0, from_scan, "blocked dual check matches scalar scan");
    }

    // Inject every fault class across block boundaries (cell 73 is inside block 2)
    // and verify blocked check returns the exact same error string as scalar scan/check.
    let base = build_clean(17, 11);
    let idx = 73_usize;

    // 1. Single NaN moment on Carrier 0.
    let mut f_nan0 = base.clone();
    f_nan0.moment_mut(19).expect("m19")[idx] = f32::NAN;
    let err = f_nan0.check_invariants_on(tol, 4).unwrap_err();
    assert!(
        format!("{err}").contains("channel 19"),
        "expected channel 19 NaN, got {err}"
    );
    assert_eq!(f_nan0.scan_invariants(tol).offending, Some(idx));

    // 2. Single -Inf moment on Carrier 1 (squares to +Inf, must not cancel).
    let mut f_inf1 = base.clone();
    f_inf1
        .plane_mut(CARRIER1_FIRST_MOMENT_PLANE + 5)
        .expect("c1 m5")[idx] = f32::NEG_INFINITY;
    let err = f_inf1.check_invariants_on(tol, 4).unwrap_err();
    assert!(
        format!("{err}").contains("channel 5"),
        "expected Carrier 1 channel 5 -Inf, got {err}"
    );
    assert_eq!(f_inf1.scan_invariants(tol).offending, Some(idx));

    // 3. Negative energy on Carrier 1.
    let mut f_neg1 = base.clone();
    f_neg1.plane_mut(CARRIER1_ENERGY_PLANE).expect("e1")[idx] = -0.01;
    let err = f_neg1.check_invariants_on(tol, 4).unwrap_err();
    assert!(
        format!("{err}").contains("INV-01"),
        "expected Carrier 1 negative energy INV-01, got {err}"
    );
    assert_eq!(f_neg1.scan_invariants(tol).offending, Some(idx));

    // 4. Error priority: cell 71 (non-empty) has Carrier 0 cone violation, cell 73 has Carrier 1 NaN ->
    // cell 71 (earlier in cell-index order) must be reported first.
    let mut f_order = base.clone();
    assert!(f_order.energy()[71] > 0.0, "cell 71 must be non-empty");
    f_order.moment_mut(0).expect("m0")[71] = f_order.energy()[71] * 2.0;
    f_order
        .plane_mut(CARRIER1_FIRST_MOMENT_PLANE)
        .expect("c1 m0")[73] = f32::NAN;
    let err = f_order.check_invariants_on(tol, 4).unwrap_err();
    assert!(
        format!("{err}").contains("INV-02"),
        "expected earlier cell 71 INV-02 before cell 73 NaN, got {err}"
    );
    assert_eq!(f_order.scan_invariants(tol).offending, Some(71));

    // 5. Same-cell Carrier 0 before Carrier 1 priority, and Carrier 1 cone violation alone.
    let mut f_same = base.clone();
    f_same.moment_mut(0).expect("m0")[idx] = f_same.energy()[idx] * 2.0;
    f_same
        .plane_mut(CARRIER1_FIRST_MOMENT_PLANE)
        .expect("c1 m0")[idx] = f32::NAN;
    let err = f_same.check_invariants_on(tol, 4).unwrap_err();
    assert!(
        format!("{err}").contains("INV-02"),
        "expected same-cell Carrier 0 INV-02 before Carrier 1 NaN, got {err}"
    );

    let mut f_cone1 = base.clone();
    let e1_73 = f_cone1.plane(CARRIER1_ENERGY_PLANE).expect("e1")[idx];
    f_cone1
        .plane_mut(CARRIER1_FIRST_MOMENT_PLANE)
        .expect("c1 m0")[idx] = e1_73 * 2.0;
    let err = f_cone1.check_invariants_on(tol, 4).unwrap_err();
    assert!(
        format!("{err}").contains("INV-02"),
        "expected Carrier 1 INV-02, got {err}"
    );
    assert_eq!(f_cone1.scan_invariants(tol).offending, Some(idx));
}

#[test]
fn stencil_flux_vectorized_widths_and_fused_dual_carrier_transport_match_serial_bit_for_bit() {
    let step = TimeStep::new(1, 32).expect("step");
    let kernel = SourceKernel::new(1.5, 3.0).expect("kernel");
    // Exercise width = 1, 2, 3 (no 4-wide chunks), 5 (3 interior), 19 (4 chunks + 1 rem),
    // and 32 (7 chunks + 2 rem) across both single-carrier flux and fused 34-plane dual-carrier.
    for &(cx, cy) in &[(1_u32, 4_u32), (2, 4), (3, 5), (5, 5), (19, 7), (32, 8)] {
        let grid = FieldGrid::new(cx, cy, f64::from(cx), f64::from(cy)).expect("grid");
        let plan0 = TransportPlan::new(&grid, step, 0.2, 0.05).expect("plan0");
        let plan1 = TransportPlan::new(&grid, step, 0.02, 0.01).expect("plan1");
        let cells = grid.cell_count() as usize;

        let mut base =
            FieldState::zeroed_dual_carrier(grid, &BufferLimits::host()).expect("dual field");
        for plane in 0..base.planes() {
            let slice = base.plane_mut(plane).expect("plane");
            for (c, slot) in slice.iter_mut().enumerate() {
                let v = ((c * 17 + plane as usize * 31) % 97) as f32 * 0.01;
                *slot = if plane == 0 || plane == CARRIER1_ENERGY_PLANE {
                    1.0 + v
                } else {
                    (v - 0.45) * 0.08
                };
            }
        }
        let mut e0 = FluxEdges::new(&grid, &BufferLimits::host()).expect("e0");
        let mut e1 = FluxEdges::new(&grid, &BufferLimits::host()).expect("e1");
        let (rx0, ry0) = plan0.ratios();
        let (rx1, ry1) = plan1.ratios();
        for c in 0..cells {
            let f = 0.25 + 0.75 * (((c * 13) % 29) as f64 / 29.0);
            e0.edge_x_mut()[c] = rx0 * f;
            e0.edge_y_mut()[c] = ry0 * (1.0 - 0.5 * f);
            e1.edge_x_mut()[c] = rx1 * (1.0 - 0.4 * f);
            e1.edge_y_mut()[c] = ry1 * f;
        }

        let phase2 = FieldStepPhase2 {
            source_kernels: None,
            carrier0_edges: Some(&e0),
            vascular: None,
            trail_plan: Some(&plan1),
            trail_edges: Some(&e1),
        };
        let mut p1 = FieldPair::from_states(
            base.clone(),
            FieldState::zeroed_dual_carrier(grid, &BufferLimits::host()).expect("next 1w"),
        )
        .expect("pair 1w");
        let mut w1 = FieldWork::new(
            &grid,
            &BufferLimits::host(),
            NonZeroUsize::new(1).expect("1"),
        )
        .expect("w1");
        let r1 = advance_field_wave2_on(&mut p1, &plan0, &kernel, &[], step, phase2, &mut w1)
            .expect("1w dual flux");
        p1.commit();

        // Compare against explicit sequential range calls (flux_checked oracle via width/workers).
        let mut ref_next =
            FieldState::zeroed_dual_carrier(grid, &BufferLimits::host()).expect("ref next");
        let mut s_ref = TransportScratch::new(&grid, &BufferLimits::host()).expect("s_ref");
        let seq0 = flife_field::transport_flux_range_on(
            &plan0,
            &e0,
            &base,
            &mut ref_next,
            base.carrier0().plane_range(),
            Some(0),
            std::slice::from_mut(&mut s_ref),
        )
        .expect("seq0")
        .expect("r0");
        let seq1 = flife_field::transport_flux_range_on(
            &plan1,
            &e1,
            &base,
            &mut ref_next,
            base.carrier1().expect("c1").plane_range(),
            Some(CARRIER1_ENERGY_PLANE as usize),
            std::slice::from_mut(&mut s_ref),
        )
        .expect("seq1")
        .expect("r1");
        assert_eq!(
            r1.transport,
            flife_field::combine_transport_reports(seq0, seq1),
            "fused vs sequential report on {cx}x{cy}"
        );
        let p1_bits: Vec<u32> = p1
            .current()
            .as_slice()
            .iter()
            .map(|v| v.to_bits())
            .collect();
        let ref_bits: Vec<u32> = ref_next.as_slice().iter().map(|v| v.to_bits()).collect();
        assert_eq!(
            p1_bits, ref_bits,
            "fused vs sequential field bits on {cx}x{cy}"
        );

        for workers in [2_usize, 4, 8, 12] {
            let mut pw = FieldPair::from_states(
                base.clone(),
                FieldState::zeroed_dual_carrier(grid, &BufferLimits::host()).expect("next w"),
            )
            .expect("pair w");
            let mut ww = FieldWork::new(
                &grid,
                &BufferLimits::host(),
                NonZeroUsize::new(workers).expect("w"),
            )
            .expect("ww");
            let rw = advance_field_wave2_on(&mut pw, &plan0, &kernel, &[], step, phase2, &mut ww)
                .expect("w dual flux");
            pw.commit();
            assert_eq!(rw, r1, "report mismatch at {workers} workers on {cx}x{cy}");
            let pw_bits: Vec<u32> = pw
                .current()
                .as_slice()
                .iter()
                .map(|v| v.to_bits())
                .collect();
            assert_eq!(
                pw_bits, p1_bits,
                "field bits mismatch at {workers} workers on {cx}x{cy}"
            );
        }
    }
}
