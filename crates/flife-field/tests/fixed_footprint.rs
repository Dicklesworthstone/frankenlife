//! FL-053: fixed-world-footprint exchange (`fixed-world-gaussian-v1`)
//! invariants, conservation, worker invariance, and grid-refinement
//! convergence against independent continuous integrals (plan §4.3, §11.6,
//! INV-01..INV-05).

#![allow(
    clippy::expect_used,
    reason = "test helpers: a failure in one is a test failure"
)]

use flife_core::ids::OrganismId;
use flife_core::ledger::{Holdings, ResidualBudget, TickLedger, Transfer};
use flife_core::{CoreError, TimeStep};
use flife_field::state::CHANNELS;
use flife_field::{
    BufferLimits, ConeTolerance, Deposits, ExchangeKernel, ExchangeScratch, FieldGrid, FieldState,
    Grants, RequestRule, apply_deposits_on, apply_grants_on, demand_with, demands_with,
    resolve_uptake_with_on_workers,
};

const TWO_PI: f64 = std::f64::consts::TAU;

/// Smooth periodic analytical energy density `ρ(x, y) > 0` on `[0, L)²`.
fn smooth_density(x: f64, y: f64, world_size: f64) -> f64 {
    let kx = TWO_PI * x / world_size;
    let ky = TWO_PI * y / world_size;
    2.5 + 0.9 * kx.cos() + 0.6 * ky.sin() + 0.4 * (kx - ky).cos()
}

/// Smooth periodic test function `φ(x, y)` for measuring deposit density
/// convergence independent of grid discretization.
fn smooth_probe(x: f64, y: f64, world_size: f64) -> f64 {
    let kx = TWO_PI * x / world_size;
    let ky = TWO_PI * y / world_size;
    1.2 + 0.5 * (2.0 * kx).sin() + 0.35 * (kx + ky).cos()
}

/// Independent continuous truncated-Gaussian integral via 256×256 midpoint
/// quadrature over `[-R, R]²`:
/// `(∫_{|d|<=R} exp(-|d|²/(2σ²)) f(x₀ + d) dA) / (∫_{|d|<=R} exp(-|d|²/(2σ²)) dA)`.
fn independent_gaussian_average(
    center: [f64; 2],
    radius: f64,
    world_size: f64,
    f: impl Fn(f64, f64, f64) -> f64,
) -> f64 {
    let sigma = radius / 3.0;
    let two_sigma2 = 2.0 * sigma * sigma;
    let r2 = radius * radius;
    let samples = 256_u32;
    let step = (2.0 * radius) / f64::from(samples);
    let mut num = 0.0;
    let mut den = 0.0;
    for iy in 0..samples {
        let dy = -radius + (f64::from(iy) + 0.5) * step;
        let y = (center[1] + dy).rem_euclid(world_size);
        for ix in 0..samples {
            let dx = -radius + (f64::from(ix) + 0.5) * step;
            let d2 = dx * dx + dy * dy;
            if d2 > r2 {
                continue;
            }
            let x = (center[0] + dx).rem_euclid(world_size);
            let w = (-d2 / two_sigma2).exp();
            num += w * f(x, y, world_size);
            den += w;
        }
    }
    num / den
}

fn fill_smooth_field(cells: u32, world_size: f64) -> FieldState {
    let grid = FieldGrid::new(cells, cells, world_size, world_size).expect("grid");
    let mut field = FieldState::zeroed(grid, &BufferLimits::host()).expect("field");
    let area = grid.cell_area();
    let hx = grid.cell_width();
    let hy = grid.cell_height();
    let mut dir = [0.0_f64; CHANNELS];
    dir[0] = 0.6;
    dir[1] = -0.8;
    for row in 0..cells {
        let y = (f64::from(row) + 0.5) * hy;
        for col in 0..cells {
            let x = (f64::from(col) + 0.5) * hx;
            let idx = grid.cell_index(col, row).expect("cell");
            let e = (smooth_density(x, y, world_size) * area) as f32;
            field.energy_mut()[idx] = e;
            for (ch, &comp) in dir.iter().enumerate() {
                if comp != 0.0 {
                    field.moment_mut(ch as u32).expect("plane")[idx] = (f64::from(e) * comp) as f32;
                }
            }
        }
    }
    field
}

/// FL-053 registry evidence: under grid refinement (`16² → 32² → 64² → 128²`)
/// at fixed physical world size (`32×32`) and fixed exchange radius `R = 2.0`,
/// `fixed-world-gaussian-v1` converges smoothly to the independent continuous
/// Gaussian integral for sampled density, deposit moments, and uptake grants,
/// whereas `grid-cic-v1`'s deposit peak density diverges as `O(1/h²)`.
#[test]
fn fixed_world_gaussian_footprint_converges_under_grid_refinement() {
    let world_size = 32.0;
    let radius = 2.0;
    let pos = [11.37, 23.81];
    let kernel = ExchangeKernel::fixed_world_gaussian(radius).expect("kernel");
    let cic = ExchangeKernel::grid_cic();
    let rule = RequestRule {
        step: TimeStep::new(1, 30).expect("step"),
        capacity_per_second: 6.0,
        half_saturation_density: 1.5,
    };

    let exact_density = independent_gaussian_average(pos, radius, world_size, smooth_density);
    let exact_probe = independent_gaussian_average(pos, radius, world_size, smooth_probe);
    let exact_request = rule.request(exact_density).expect("exact request");

    let mut density_errors = Vec::new();
    let mut request_errors = Vec::new();
    let mut deposit_probe_errors = Vec::new();
    let mut fixed_peak_densities = Vec::new();
    let mut cic_peak_densities = Vec::new();

    for &n in &[16_u32, 32, 64, 128] {
        let field = fill_smooth_field(n, world_size);
        let grid = *field.grid();

        // 1. Sampled local density and A.1 saturating request convergence.
        let sampled = kernel.density(&field, pos).expect("density");
        density_errors.push((sampled - exact_density).abs());

        let dem = demand_with(&field, &rule, kernel, OrganismId::new(1), pos).expect("demand");
        request_errors.push((dem.amount - exact_request).abs());

        // 2. Deposit density convergence (fixed energy Q = 10.0).
        let offer = 10.0;
        let mut dir = [0.0; CHANNELS];
        dir[0] = 1.0;

        let mut fixed_field = FieldState::zeroed(grid, &BufferLimits::host()).expect("zero field");
        let mut fixed_dep = Deposits::with_capacity_for(&grid, 1, kernel, &BufferLimits::host())
            .expect("fixed deposits");
        fixed_dep
            .push_with(&grid, kernel, OrganismId::new(1), pos, offer, offer, &dir)
            .expect("push fixed");
        let fixed_sum = apply_deposits_on(&mut fixed_field, &mut fixed_dep, 1).expect("apply");
        assert!(
            fixed_sum.applied <= offer && (offer - fixed_sum.applied).abs() < 1e-4,
            "deposit conservation on {n}x{n}: applied={} offer={offer}",
            fixed_sum.applied
        );
        let area = grid.cell_area();
        let mut peak_fixed = 0.0_f64;
        let mut probe_integral = 0.0_f64;
        for row in 0..n {
            let y = (f64::from(row) + 0.5) * grid.cell_height();
            for col in 0..n {
                let x = (f64::from(col) + 0.5) * grid.cell_width();
                let idx = grid.cell_index(col, row).expect("cell");
                let e = f64::from(fixed_field.energy()[idx]);
                if e > 0.0 {
                    peak_fixed = peak_fixed.max(e / area);
                    probe_integral += e * smooth_probe(x, y, world_size);
                }
            }
        }
        fixed_peak_densities.push(peak_fixed);
        deposit_probe_errors.push((probe_integral / offer - exact_probe).abs());

        // Compare against `grid-cic-v1` peak deposit density scaling with `1/h²`.
        let mut cic_field = FieldState::zeroed(grid, &BufferLimits::host()).expect("zero field");
        let mut cic_dep =
            Deposits::with_capacity_for(&grid, 1, cic, &BufferLimits::host()).expect("cic dep");
        cic_dep
            .push_with(&grid, cic, OrganismId::new(1), pos, offer, offer, &dir)
            .expect("push cic");
        apply_deposits_on(&mut cic_field, &mut cic_dep, 1).expect("apply cic");
        let peak_cic = cic_field
            .energy()
            .iter()
            .fold(0.0_f64, |m, &e| m.max(f64::from(e) / area));
        cic_peak_densities.push(peak_cic);
    }

    // Strict monotone error reduction across 32 -> 64 -> 128 and tight final accuracy.
    assert!(
        density_errors[3] < density_errors[2] && density_errors[2] < density_errors[1],
        "density errors did not shrink under refinement: {density_errors:?}"
    );
    assert!(
        density_errors[3] < 1.5e-3,
        "128x128 density error too large: {}",
        density_errors[3]
    );
    assert!(
        request_errors[3] < request_errors[2] && request_errors[2] < request_errors[1],
        "request errors did not shrink under refinement: {request_errors:?}"
    );
    assert!(
        deposit_probe_errors[3] < deposit_probe_errors[2]
            && deposit_probe_errors[2] < deposit_probe_errors[1],
        "deposit probe errors did not shrink under refinement: {deposit_probe_errors:?}"
    );
    assert!(
        deposit_probe_errors[3] < 2e-3,
        "128x128 deposit probe error too large: {}",
        deposit_probe_errors[3]
    );

    // Fixed-world peak deposit density stabilizes near the continuous Gaussian
    // peak `Q / (2π σ² (1 - exp(-4.5)))`, while `grid-cic-v1` blows up by >20x.
    let sigma = radius / 3.0;
    let exact_peak = 10.0 / (TWO_PI * sigma * sigma * (1.0 - (-4.5_f64).exp()));
    assert!(
        (fixed_peak_densities[3] - exact_peak).abs() / exact_peak < 0.03,
        "128x128 fixed peak {} vs continuous peak {exact_peak}",
        fixed_peak_densities[3]
    );
    assert!(
        cic_peak_densities[3] / cic_peak_densities[0] > 20.0,
        "CIC peak should diverge as O(1/h²): {cic_peak_densities:?}"
    );
}

#[test]
fn fixed_world_gaussian_footprint_obeys_normalization_torus_invariance_and_bounds() {
    let grid = FieldGrid::new(64, 64, 32.0, 32.0).expect("grid");
    let kernel = ExchangeKernel::fixed_world_gaussian(2.0).expect("kernel");
    let mut scratch = ExchangeScratch::for_kernel(kernel, &grid).expect("scratch");
    let initial_bytes = scratch.allocated_bytes();

    // Seam-straddling position (dyadic coordinates so shifting by multiples of
    // L = 32.0 is exact in IEEE-754 binary64).
    kernel
        .footprint_into(&grid, [0.125, 31.875], &mut scratch)
        .expect("seam footprint");
    assert_eq!(
        scratch.allocated_bytes(),
        initial_bytes,
        "reserved ExchangeScratch must not grow"
    );
    let seam_cells: Vec<(usize, f64)> = scratch.cells().to_vec();
    assert!(!seam_cells.is_empty());
    assert!(seam_cells.len() <= kernel.max_cells(&grid));
    assert!(
        seam_cells.windows(2).all(|w| w[0].0 < w[1].0),
        "cells must be strictly ascending"
    );
    assert!(seam_cells.iter().all(|&(_, w)| w.is_finite() && w > 0.0));
    let weight_sum: f64 = seam_cells.iter().map(|&(_, w)| w).sum();
    assert!(
        (weight_sum - 1.0).abs() < 1e-13,
        "weights sum to {weight_sum}"
    );

    // Periodic wrap invariance: shifting by exact multiples of (L_x, L_y) is bit-identical.
    kernel
        .footprint_into(&grid, [0.125 + 64.0, 31.875 - 96.0], &mut scratch)
        .expect("wrapped footprint");
    assert_eq!(scratch.cells(), seam_cells.as_slice());

    // Interior translation invariance: shifting by an exact integer number of cells
    // preserves the multiset of weights up to cell-index summation-order rounding (<= 1e-14).
    kernel
        .footprint_into(&grid, [16.125, 15.875], &mut scratch)
        .expect("interior footprint");
    let mut seam_weights: Vec<f64> = seam_cells.iter().map(|&(_, w)| w).collect();
    let mut interior_weights: Vec<f64> = scratch.cells().iter().map(|&(_, w)| w).collect();
    seam_weights.sort_by(f64::total_cmp);
    interior_weights.sort_by(f64::total_cmp);
    assert_eq!(seam_weights.len(), interior_weights.len());
    for (a, b) in seam_weights.iter().zip(&interior_weights) {
        assert!(
            (a - b).abs() < 1e-14,
            "seam vs interior weight mismatch: {a:e} vs {b:e}"
        );
    }

    // Tiny radius at a cell corner covers no cell center and is explicitly refused.
    let tiny = ExchangeKernel::fixed_world_gaussian(0.05).expect("tiny kernel");
    let err = tiny
        .footprint_into(&grid, [0.0, 0.0], &mut scratch)
        .expect_err("empty support must be refused");
    assert!(matches!(err, CoreError::NumericalInvalid { .. }), "{err:?}");
}

#[test]
fn fixed_world_gaussian_uptake_and_deposits_conserve_and_match_across_workers() {
    let grid = FieldGrid::new(64, 64, 32.0, 32.0).expect("grid");
    let kernel = ExchangeKernel::fixed_world_gaussian(2.0).expect("kernel");
    let rule = RequestRule {
        step: TimeStep::new(1, 30).expect("step"),
        capacity_per_second: 12.0,
        half_saturation_density: 1.0,
    };
    let field = fill_smooth_field(64, 32.0);
    let organism_count = 160_u64;
    let requesters: Vec<(OrganismId, [f64; 2])> = (0..organism_count)
        .map(|i| {
            let x = ((i as f64 * 1.732_050_8) % 32.0).abs();
            let y = ((i as f64 * 2.236_067_9) % 32.0).abs();
            (OrganismId::new(i), [x, y])
        })
        .collect();

    let mut dem = Vec::new();
    demands_with(&field, &rule, kernel, requesters.iter().copied(), &mut dem).expect("demands");
    assert_eq!(dem.len(), organism_count as usize);

    // 1-worker vs 4-worker uptake resolution + application must be bit-identical
    // and allocate zero extra capacity in steady state.
    let mut grants_1 =
        Grants::with_capacity_for(&grid, organism_count, kernel, &BufferLimits::host())
            .expect("grants_1");
    let alloc_before = grants_1.allocated();
    let sum_1 =
        resolve_uptake_with_on_workers(&field, kernel, &dem, &mut grants_1, 1).expect("resolve 1");
    assert_eq!(
        grants_1.allocated(),
        alloc_before,
        "steady-state uptake must not grow reserved buffers"
    );

    let mut grants_4 =
        Grants::with_capacity_for(&grid, organism_count, kernel, &BufferLimits::host())
            .expect("grants_4");
    let sum_4 =
        resolve_uptake_with_on_workers(&field, kernel, &dem, &mut grants_4, 4).expect("resolve 4");
    assert_eq!(sum_1, sum_4);
    assert_eq!(grants_1.packets(), grants_4.packets());
    assert_eq!(grants_1.debits(), grants_4.debits());
    assert_eq!(grants_1.totals(), grants_4.totals());

    let mut field_1 = field.clone();
    let mut field_4 = field.clone();
    let debited_1 = apply_grants_on(&mut field_1, &mut grants_1, 1).expect("apply 1");
    let debited_4 = apply_grants_on(&mut field_4, &mut grants_4, 4).expect("apply 4");
    assert_eq!(debited_1.to_bits(), debited_4.to_bits());
    assert_eq!(
        field_1
            .as_slice()
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        field_4
            .as_slice()
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
    );

    // INV-01, INV-02, INV-04 checks after uptake.
    let cone = ConeTolerance::new(1e-6, 1e-5).expect("cone");
    field_1
        .check_invariants(cone)
        .expect("invariants after uptake");
    for debit in grants_1.debits() {
        assert!(debit.energy_after >= 0.0 && debit.energy_after <= debit.energy_before);
        assert!(debit.granted <= f64::from(debit.energy_before));
    }

    // Paid deposits on 1 vs 4 workers: bit-identical, zero steady-state allocation,
    // ledger closure, and INV-01/INV-02 preserved.
    let mut dir = [0.0; CHANNELS];
    dir[3] = 0.8;
    dir[7] = -0.6;
    let mut dep_1 =
        Deposits::with_capacity_for(&grid, organism_count, kernel, &BufferLimits::host())
            .expect("dep_1");
    let dep_alloc_before = dep_1.allocated();
    let mut dep_4 =
        Deposits::with_capacity_for(&grid, organism_count, kernel, &BufferLimits::host())
            .expect("dep_4");
    for &(id, pos) in &requesters {
        let paid = 0.5 + (id.get() % 5) as f64 * 0.25;
        let offered = 0.8 * paid;
        dep_1
            .push_with(&grid, kernel, id, pos, paid, offered, &dir)
            .expect("push 1");
        dep_4
            .push_with(&grid, kernel, id, pos, paid, offered, &dir)
            .expect("push 4");
    }
    let opening = Holdings::new(field_1.energy_total(), 1_000.0).expect("opening");
    let mut ledger = TickLedger::open(flife_core::Tick::new(0), opening, ResidualBudget::EXACT);
    let dsum_1 = apply_deposits_on(&mut field_1, &mut dep_1, 1).expect("dep apply 1");
    let dsum_4 = apply_deposits_on(&mut field_4, &mut dep_4, 4).expect("dep apply 4");
    assert_eq!(dep_1.allocated(), dep_alloc_before);
    assert_eq!(dsum_1, dsum_4);
    assert!(dsum_1.applied <= dsum_1.offered && dsum_1.offered <= dsum_1.paid);
    assert_eq!(
        field_1
            .as_slice()
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        field_4
            .as_slice()
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
    );
    dsum_1
        .record(&mut ledger, Transfer::Emission)
        .expect("record emission transfer");
    field_1
        .check_invariants(cone)
        .expect("invariants after deposits");

    // Extreme tiny subnormal offer never creates energy (`Σ q_c <= offered`).
    let mut tiny_dep =
        Deposits::with_capacity_for(&grid, 1, kernel, &BufferLimits::host()).expect("tiny dep");
    let subnormal = f64::MIN_POSITIVE * 0.25;
    tiny_dep
        .push_with(
            &grid,
            kernel,
            OrganismId::new(999),
            [12.3, 14.7],
            subnormal,
            subnormal,
            &dir,
        )
        .expect("push subnormal");
    let sum_packets: f64 = tiny_dep.entry_packets(0).map(|(_, a)| a).sum();
    assert!(
        sum_packets <= subnormal,
        "subnormal split overspent: {sum_packets:e} > {subnormal:e}"
    );
}
