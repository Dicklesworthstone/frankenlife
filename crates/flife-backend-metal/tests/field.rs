//! FL-031: phase 2 on Metal is [`advance_field_on`], bit for bit: every
//! value of the candidate field and every F64 term of its report, across
//! grids, substeps, decay, source supports from a few cells to the whole
//! torus, centers on the seams, and fields holding zeros, subnormals and
//! both signs of moment.

use std::num::NonZeroUsize;

use flife_backend_metal::MetalField;
use flife_core::clock::Tick;
use flife_core::ids::SourceId;
use flife_core::rng::SemanticRng;
use flife_core::{CoreError, TimeStep};
use flife_field::grid::FIELD_PLANES;
use flife_field::{
    BarrierMode, BufferLimits, DepositReport, FieldBackend, FieldGrid, FieldPair, FieldState,
    FieldStepOptions, FieldStepReport, FieldWork, FluxEdges, OasisSchedule, Source, SourceKernel,
    TightJunctions, TransportPlan, TransportScratch, advance_field_on, advance_field_v2_on,
    transport_flux_range_on,
};

const LIMITS: BufferLimits = BufferLimits::new(1 << 30);

/// `None` off macOS, where `open` must refuse with its typed error.
#[expect(clippy::panic, reason = "test helper")]
fn metal() -> Option<MetalField> {
    match MetalField::open() {
        Ok(field) => Some(field),
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => None,
        Err(error) => panic!("this host should have Metal: {error}"),
    }
}

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1_u64 << 53) as f64
    }

    /// A field value of either sign over many magnitudes, with zeros and
    /// subnormals among them.
    fn value(&mut self, signed: bool) -> f32 {
        let magnitude = match self.next() % 16 {
            0 => 0.0,
            1 => f32::from_bits((self.next() % 0x007F_FFFF) as u32 + 1),
            2 => f32::MIN_POSITIVE * self.unit() as f32,
            3 => (self.unit() * 1.0e-30) as f32,
            4 => (self.unit() * 1.0e6) as f32,
            _ => (self.unit() * 4.0) as f32,
        };
        if signed && self.next().is_multiple_of(2) {
            -magnitude
        } else {
            magnitude
        }
    }
}

#[expect(clippy::expect_used, reason = "test helper")]
fn random_pair(grid: FieldGrid, rng: &mut Rng) -> FieldPair {
    let mut pair = FieldPair::zeroed(grid, &LIMITS).expect("pair");
    let current = pair.current_mut();
    for plane in 0..FIELD_PLANES {
        for value in current.plane_mut(plane).expect("plane") {
            *value = rng.value(plane != 0);
        }
    }
    pair
}

fn report_bits(report: &FieldStepReport) -> Vec<u64> {
    let transport = &report.transport;
    let mut bits = vec![
        u64::from(transport.substeps),
        transport.energy_before.to_bits(),
        transport.energy_transported.to_bits(),
        transport.energy_after.to_bits(),
        transport.decay_removed.to_bits(),
        transport.transport_residual.to_bits(),
        report.injected_declared.to_bits(),
        report.injected_applied.to_bits(),
        report.energy_after.to_bits(),
        report.residual.to_bits(),
    ];
    for DepositReport {
        source,
        support_cells,
        declared,
        normalized_total,
        applied,
    } in &report.deposits
    {
        bits.extend([
            u64::from(source.get()),
            *support_cells as u64,
            declared.to_bits(),
            normalized_total.to_bits(),
            applied.to_bits(),
        ]);
    }
    bits
}

/// One case: a grid, a plan, a kernel and sources.
struct Case {
    name: &'static str,
    grid: FieldGrid,
    plan: TransportPlan,
    kernel: SourceKernel,
    sources: Vec<Source>,
    step: TimeStep,
}

#[expect(clippy::expect_used, reason = "test helper")]
fn case(
    name: &'static str,
    cells: [u32; 2],
    world: [f64; 2],
    diffusion_decay: [f64; 2],
    sigma_cutoff: [f64; 2],
    centers: &[[f64; 2]],
    rng: &mut Rng,
) -> Case {
    let grid = FieldGrid::new(cells[0], cells[1], world[0], world[1]).expect("grid");
    let step = TimeStep::new(1, 128).expect("step");
    let plan =
        TransportPlan::new(&grid, step, diffusion_decay[0], diffusion_decay[1]).expect("plan");
    let kernel = SourceKernel::new(sigma_cutoff[0], sigma_cutoff[1]).expect("kernel");
    let sources = centers
        .iter()
        .enumerate()
        .map(|(index, &center)| {
            let mut direction = [0.0; 32];
            // Some channels zero (skipped), the rest a direction of norm < 1.
            for component in direction.iter_mut() {
                if !rng.next().is_multiple_of(3) {
                    *component = (rng.unit() - 0.5) * 0.3;
                }
            }
            Source::new(
                SourceId::new(index as u32),
                center,
                direction,
                rng.unit() * 2_000.0,
            )
            .expect("source")
        })
        .collect();
    Case {
        name,
        grid,
        plan,
        kernel,
        sources,
        step,
    }
}

#[test]
fn the_metal_step_is_the_reference_step_bit_for_bit() {
    let Some(mut device) = metal() else {
        return;
    };
    let mut rng = Rng(0x0F1E_2D3C_4B5A_6978);
    let random_centers = |rng: &mut Rng, count: usize, world: [f64; 2]| -> Vec<[f64; 2]> {
        (0..count)
            .map(|_| [rng.unit() * world[0], rng.unit() * world[1]])
            .collect()
    };
    let mut cases = Vec::new();
    // The preset's scale: one substep, decay, eight sources of radius 40.
    let centers = random_centers(&mut rng, 8, [128.0, 128.0]);
    cases.push(case(
        "preset scale",
        [128, 128],
        [128.0, 128.0],
        [0.15, 0.01],
        [10.0, 4.0],
        &centers,
        &mut rng,
    ));
    cases.push(case(
        "preset plan, no sources",
        [128, 128],
        [128.0, 128.0],
        [0.15, 0.01],
        [10.0, 4.0],
        &[],
        &mut rng,
    ));
    // Centers on and near the seams and corners of a non-square grid with
    // non-unit cells.
    cases.push(case(
        "seams",
        [96, 40],
        [48.0, 30.0],
        [0.05, 0.2],
        [1.7, 3.0],
        &[
            [0.0, 0.0],
            [47.999_999, 29.999_999],
            [24.0, 0.0],
            [0.0, 15.0],
            [0.375, 0.375],
            [47.625, 0.75],
        ],
        &mut rng,
    ));
    // Substeps: a diffusion past the stability bound, no decay, so the
    // energy plane's decay sum is the exact zero.
    let centers = random_centers(&mut rng, 3, [32.0, 32.0]);
    for (name, diffusion) in [("two substeps", 40.0), ("five substeps", 130.0)] {
        cases.push(case(
            name,
            [32, 32],
            [32.0, 32.0],
            [diffusion, 0.0],
            [2.0, 3.0],
            &centers,
            &mut rng,
        ));
    }
    // A support covering the whole torus, and one of a single cell.
    cases.push(case(
        "whole torus",
        [24, 20],
        [24.0, 20.0],
        [0.3, 0.05],
        [30.0, 4.0],
        &random_centers(&mut rng, 2, [24.0, 20.0]),
        &mut rng,
    ));
    cases.push(case(
        "one cell",
        [64, 64],
        [64.0, 64.0],
        [0.3, 0.05],
        [0.1, 4.0],
        &[[10.5, 20.5], [63.5, 0.5]],
        &mut rng,
    ));
    // Centers on cell centers with an integer radius (σ 10 × cutoff 4):
    // the candidates 40 cells away along an axis lie at exactly the radius,
    // where the reference's support still includes them.
    cases.push(case(
        "radius boundary",
        [128, 128],
        [128.0, 128.0],
        [0.15, 0.01],
        [10.0, 4.0],
        &[[10.5, 20.5], [64.5, 64.5]],
        &mut rng,
    ));
    // No sources.
    cases.push(case(
        "no sources",
        [16, 8],
        [16.0, 8.0],
        [0.5, 0.5],
        [1.0, 2.0],
        &[],
        &mut rng,
    ));

    let mut compared = 0_usize;
    for case in &cases {
        for round in 0..3 {
            let mut reference = random_pair(case.grid, &mut rng);
            let mut accelerated = FieldPair::zeroed(case.grid, &LIMITS).expect("pair");
            accelerated
                .current_mut()
                .copy_from(reference.current())
                .expect("copy");
            let mut work = FieldWork::new(&case.grid, &LIMITS, NonZeroUsize::new(4).expect("4"))
                .expect("work");
            let expected = advance_field_on(
                &mut reference,
                &case.plan,
                &case.kernel,
                &case.sources,
                case.step,
                &mut work,
            )
            .expect("the reference steps");
            let got = device
                .advance(
                    &mut accelerated,
                    &case.plan,
                    &case.kernel,
                    &case.sources,
                    case.step,
                )
                .expect("the metal field steps");
            let (want, have) = (
                reference.stage().1.as_slice(),
                accelerated.stage().1.as_slice(),
            );
            let differing = want
                .iter()
                .zip(have)
                .filter(|(a, b)| a.to_bits() != b.to_bits())
                .count();
            assert_eq!(
                differing,
                0,
                "{} round {round}: {differing} of {} values differ",
                case.name,
                want.len()
            );
            assert_eq!(
                report_bits(&expected),
                report_bits(&got),
                "{} round {round}: the reports differ",
                case.name
            );
            compared += want.len();
        }
    }
    assert!(compared > 1_500_000, "compared {compared} values");
}

/// A source whose support has a weight beyond F64's range of `exp` is
/// still the reference's: the device's weight for it is the reference's
/// zero, and the cell stays in the support.
#[test]
fn a_support_whose_far_weights_underflow_is_still_the_reference() {
    let Some(mut device) = metal() else {
        return;
    };
    let mut rng = Rng(0x5EED);
    // Radius 45σ: |d|²/2σ² reaches 1012, past exp's underflow at 746.
    let case = case(
        "underflowing weights",
        [64, 64],
        [64.0, 64.0],
        [0.2, 0.1],
        [0.7, 45.0],
        &[[31.3, 12.9], [0.2, 63.9]],
        &mut rng,
    );
    let mut reference = random_pair(case.grid, &mut rng);
    let mut accelerated = FieldPair::zeroed(case.grid, &LIMITS).expect("pair");
    accelerated
        .current_mut()
        .copy_from(reference.current())
        .expect("copy");
    let mut work =
        FieldWork::new(&case.grid, &LIMITS, NonZeroUsize::new(1).expect("1")).expect("work");
    let expected = advance_field_on(
        &mut reference,
        &case.plan,
        &case.kernel,
        &case.sources,
        case.step,
        &mut work,
    )
    .expect("reference");
    let got = device
        .advance(
            &mut accelerated,
            &case.plan,
            &case.kernel,
            &case.sources,
            case.step,
        )
        .expect("metal");
    assert!(
        expected
            .deposits
            .iter()
            .all(|deposit| deposit.support_cells > 3_000),
        "the supports should span most of the grid: {:?}",
        expected.deposits
    );
    let (want, have) = (
        reference.stage().1.as_slice(),
        accelerated.stage().1.as_slice(),
    );
    assert!(
        want.iter()
            .zip(have)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
    assert_eq!(report_bits(&expected), report_bits(&got));
}

/// A committed field holding a non-finite value is refused (the world then
/// redoes the step on the reference), never silently diverged from.
#[test]
fn a_non_finite_operand_is_refused() {
    let Some(mut device) = metal() else {
        return;
    };
    let mut rng = Rng(7);
    let case = case(
        "non-finite",
        [16, 16],
        [16.0, 16.0],
        [0.2, 0.1],
        [1.0, 3.0],
        &[[8.0, 8.0]],
        &mut rng,
    );
    for (plane, value) in [(0, f32::INFINITY), (5, f32::NAN)] {
        let mut pair = random_pair(case.grid, &mut rng);
        pair.current_mut().plane_mut(plane).expect("plane")[37] = value;
        let refused = device.advance(
            &mut pair,
            &case.plan,
            &case.kernel,
            &case.sources,
            case.step,
        );
        assert!(
            matches!(refused, Err(CoreError::NumericalInvalid { .. })),
            "plane {plane}: {refused:?}"
        );
    }
    // The device is still usable afterwards.
    let mut pair = random_pair(case.grid, &mut rng);
    device
        .advance(
            &mut pair,
            &case.plan,
            &case.kernel,
            &case.sources,
            case.step,
        )
        .expect("a finite field steps");
}

/// Moment planes of negative zeros keep them through diffusion and decay,
/// and a source whose direction is zero in a channel leaves that channel's
/// negative zeros alone, as the reference does: adding its zero amount
/// would make them positive.
#[test]
fn a_zero_component_leaves_negative_zero_moments_negative() {
    let Some(mut device) = metal() else {
        return;
    };
    let mut rng = Rng(0x00DE_C0DE);
    let case = case(
        "negative zero moments",
        [48, 40],
        [48.0, 40.0],
        [0.2, 0.05],
        [3.0, 3.0],
        &[[12.3, 17.9], [40.1, 3.3]],
        &mut rng,
    );
    let mut reference = random_pair(case.grid, &mut rng);
    for plane in 1..FIELD_PLANES {
        reference
            .current_mut()
            .plane_mut(plane)
            .expect("plane")
            .fill(-0.0);
    }
    let mut accelerated = FieldPair::zeroed(case.grid, &LIMITS).expect("pair");
    accelerated
        .current_mut()
        .copy_from(reference.current())
        .expect("copy");
    let mut work =
        FieldWork::new(&case.grid, &LIMITS, NonZeroUsize::new(2).expect("2")).expect("work");
    advance_field_on(
        &mut reference,
        &case.plan,
        &case.kernel,
        &case.sources,
        case.step,
        &mut work,
    )
    .expect("the reference steps");
    device
        .advance(
            &mut accelerated,
            &case.plan,
            &case.kernel,
            &case.sources,
            case.step,
        )
        .expect("the metal field steps");
    let want = reference.stage().1.as_slice();
    let negative_zeros = want
        .iter()
        .filter(|value| value.to_bits() == (-0.0_f32).to_bits())
        .count();
    let cells = want.len() / FIELD_PLANES as usize;
    // The setup holds: the negative zeros survived the step on the
    // reference (the mutation run shows the skipped channels among them).
    assert!(
        negative_zeros > cells,
        "only {negative_zeros} negative zeros"
    );
    let differing = want
        .iter()
        .zip(accelerated.stage().1.as_slice())
        .filter(|(a, b)| a.to_bits() != b.to_bits())
        .count();
    assert_eq!(differing, 0, "{differing} values differ");
}

/// Many random worlds: grid shapes and cell sizes, kernels from a few cells
/// to the whole torus, one to eight sources anywhere, substeps and decay.
/// A device result that depends on how the kernels happen to compile would
/// show here as a differing value.
#[test]
fn random_worlds_step_as_the_reference() {
    let Some(mut device) = metal() else {
        return;
    };
    let mut rng = Rng(0xC0FF_EE00_D15E_A5E5);
    for world in 0..48 {
        let cells = [8 + (rng.next() % 90) as u32, 8 + (rng.next() % 90) as u32];
        let size = [
            f64::from(cells[0]) * (0.25 + rng.unit() * 2.0),
            f64::from(cells[1]) * (0.25 + rng.unit() * 2.0),
        ];
        let sigma = 0.2 + rng.unit() * 6.0;
        let cutoff = 0.5 + rng.unit() * 5.0;
        let diffusion = rng.unit() * if world % 4 == 0 { 300.0 } else { 1.0 };
        let decay = if world % 3 == 0 { 0.0 } else { rng.unit() };
        let count = 1 + (rng.next() % 8) as usize;
        let centers: Vec<[f64; 2]> = (0..count)
            .map(|_| [rng.unit() * size[0], rng.unit() * size[1]])
            .collect();
        let case = case(
            "random",
            cells,
            size,
            [diffusion, decay],
            [sigma, cutoff],
            &centers,
            &mut rng,
        );
        let mut reference = random_pair(case.grid, &mut rng);
        let mut accelerated = FieldPair::zeroed(case.grid, &LIMITS).expect("pair");
        accelerated
            .current_mut()
            .copy_from(reference.current())
            .expect("copy");
        let mut work =
            FieldWork::new(&case.grid, &LIMITS, NonZeroUsize::new(2).expect("2")).expect("work");
        let expected = advance_field_on(
            &mut reference,
            &case.plan,
            &case.kernel,
            &case.sources,
            case.step,
            &mut work,
        );
        let got = device.advance(
            &mut accelerated,
            &case.plan,
            &case.kernel,
            &case.sources,
            case.step,
        );
        match (expected, got) {
            (Ok(expected), Ok(got)) => {
                let (want, have) = (
                    reference.stage().1.as_slice(),
                    accelerated.stage().1.as_slice(),
                );
                let differing = want
                    .iter()
                    .zip(have)
                    .filter(|(a, b)| a.to_bits() != b.to_bits())
                    .count();
                assert_eq!(differing, 0, "world {world}: {differing} values differ");
                assert_eq!(report_bits(&expected), report_bits(&got), "world {world}");
            }
            // A plan or support the reference refuses, the device refuses too.
            (Err(_), Err(_)) => {}
            (expected, got) => panic!("world {world}: reference {expected:?}, metal {got:?}"),
        }
    }
}

/// `flife-ze4`: `MetalField` matches CPU `advance_field_on` bit for bit under
/// `normalized-truncated-minimum-image-gaussian-fused-v1` (multi-source fused
/// F64 accumulation on both energy and moment planes).
#[test]
fn fused_source_kernel_matches_cpu_bit_for_bit() {
    let Some(mut device) = metal() else {
        return;
    };
    let mut rng = Rng(0x00F0_5ED1_1234_5678);
    let centers = [
        [8.3, 12.7],
        [10.1, 14.2],
        [24.9, 6.4],
        [31.5, 29.8],
        [2.2, 18.6],
        [15.4, 21.1],
    ];
    let base = case(
        "fused sources",
        [32, 32],
        [32.0, 32.0],
        [0.327_384, 0.008_989_29],
        [4.0, 4.0],
        &centers,
        &mut rng,
    );
    let fused = SourceKernel::new_fused(4.0, 4.0).expect("fused kernel");
    let mut reference = random_pair(base.grid, &mut rng);
    let mut accelerated = FieldPair::zeroed(base.grid, &LIMITS).expect("pair");
    accelerated
        .current_mut()
        .copy_from(reference.current())
        .expect("copy");
    let mut work =
        FieldWork::new(&base.grid, &LIMITS, NonZeroUsize::new(4).expect("4")).expect("work");
    work.reserve_supports(base.sources.len(), &fused)
        .expect("reserve");
    for tick in 0..8 {
        let expected = advance_field_on(
            &mut reference,
            &base.plan,
            &fused,
            &base.sources,
            base.step,
            &mut work,
        )
        .expect("cpu fused step");
        let got = device
            .advance(
                &mut accelerated,
                &base.plan,
                &fused,
                &base.sources,
                base.step,
            )
            .expect("metal fused step");
        let (want, have) = (
            reference.stage().1.as_slice(),
            accelerated.stage().1.as_slice(),
        );
        let differing = want
            .iter()
            .zip(have)
            .filter(|(a, b)| a.to_bits() != b.to_bits())
            .count();
        assert_eq!(differing, 0, "tick {tick}: {differing} values differ");
        assert_eq!(report_bits(&expected), report_bits(&got), "tick {tick}");
        reference.commit();
        accelerated.commit();
    }
}

/// `V2-02` (`oases`): 64 ticks spanning zero-power dormant transitions,
/// peak-power blooms, and heterogeneous per-source `σ_j` on BOTH non-fused
/// and fused kernels match CPU `advance_field_v2_on` bit-for-bit on every
/// plane and report field, plus all-dormant and negative refusal checks.
#[test]
fn metal_field_matches_cpu_on_pulsing_heterogeneous_oases() {
    let Some(mut device) = metal() else {
        return;
    };
    let mut rng = Rng(0x2026_0202);
    let grid = FieldGrid::new(64, 64, 64.0, 64.0).expect("grid");
    let step = TimeStep::new(1, 32).expect("step");
    let plan = TransportPlan::new(&grid, step, 0.25, 0.02).expect("plan");
    let centers = [[10.25, 14.75], [48.5, 12.25], [16.0, 52.5], [55.75, 49.0]];
    let mut base_sources: Vec<Source> = centers
        .iter()
        .enumerate()
        .map(|(index, &center)| {
            let mut direction = [0.0; 32];
            for (c, slot) in direction.iter_mut().enumerate() {
                if (c + index) % 3 != 0 {
                    *slot = (rng.unit() - 0.5) * 0.28;
                }
            }
            Source::new(SourceId::new(index as u32), center, direction, 120.0).expect("source")
        })
        .collect();
    let schedule = OasisSchedule::new(1.0, 0.0, 1.5, 4.0).expect("oasis schedule");
    let seed_rng = SemanticRng::new(0x0A51_5002);

    for fused in [false, true] {
        let base_kernel = if fused {
            SourceKernel::new_fused(2.5, 4.0).expect("fused")
        } else {
            SourceKernel::new(2.5, 4.0).expect("non-fused")
        };
        let kernels = schedule
            .seed_kernels(&seed_rng, &base_kernel, &base_sources)
            .expect("seed kernels");
        let mut reference = random_pair(grid, &mut rng);
        let mut accelerated = FieldPair::zeroed(grid, &LIMITS).expect("pair");
        accelerated
            .current_mut()
            .copy_from(reference.current())
            .expect("copy");
        let mut work =
            FieldWork::new(&grid, &LIMITS, NonZeroUsize::new(4).expect("4")).expect("work");
        work.reserve_supports_for_kernels(&kernels)
            .expect("reserve supports");

        let mut power_scratch = Vec::new();
        let mut dormant_seen = 0_usize;
        for tick in 0..64_u64 {
            schedule
                .apply_powers(
                    Tick::new(tick),
                    step,
                    120.0,
                    &mut base_sources,
                    &mut power_scratch,
                )
                .expect("apply powers");
            dormant_seen += base_sources.iter().filter(|s| s.power() == 0.0).count();
            let options = FieldStepOptions {
                source_kernels: Some(&kernels),
                flux_edges: None,
            };
            let expected = advance_field_v2_on(
                &mut reference,
                &plan,
                &base_kernel,
                &base_sources,
                step,
                options,
                &mut work,
            )
            .expect("cpu oasis step");
            let got = device
                .advance_v2(
                    &mut accelerated,
                    &plan,
                    &base_kernel,
                    &base_sources,
                    step,
                    options,
                )
                .expect("metal oasis step");
            let (want, have) = (
                reference.stage().1.as_slice(),
                accelerated.stage().1.as_slice(),
            );
            let differing = want
                .iter()
                .zip(have)
                .filter(|(a, b)| a.to_bits() != b.to_bits())
                .count();
            assert_eq!(
                differing, 0,
                "fused={fused} tick={tick}: {differing} values differ"
            );
            assert_eq!(
                report_bits(&expected),
                report_bits(&got),
                "fused={fused} tick={tick}: reports differ"
            );
            reference.commit();
            accelerated.commit();
        }
        assert!(
            dormant_seen > 20,
            "fused={fused}: expected dormant zero-power source ticks, saw {dormant_seen}"
        );

        // All-dormant step (every source power == 0.0 -> entries == 0) matches bit-for-bit.
        let dormant_sources: Vec<Source> = base_sources
            .iter()
            .map(|s| s.with_power(0.0).expect("zero power"))
            .collect();
        let options = FieldStepOptions {
            source_kernels: Some(&kernels),
            flux_edges: None,
        };
        let expected = advance_field_v2_on(
            &mut reference,
            &plan,
            &base_kernel,
            &dormant_sources,
            step,
            options,
            &mut work,
        )
        .expect("cpu all-dormant");
        let got = device
            .advance_v2(
                &mut accelerated,
                &plan,
                &base_kernel,
                &dormant_sources,
                step,
                options,
            )
            .expect("metal all-dormant");
        assert!(
            reference
                .stage()
                .1
                .as_slice()
                .iter()
                .zip(accelerated.stage().1.as_slice())
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        assert_eq!(report_bits(&expected), report_bits(&got));
    }

    // Negative refusals: mismatched kernel slice length and mixed fused mode.
    let base_kernel = SourceKernel::new(2.5, 4.0).expect("base");
    let wrong_mode = SourceKernel::new_fused(2.5, 4.0).expect("fused");
    let mut pair = random_pair(grid, &mut rng);
    assert!(matches!(
        device.advance_v2(
            &mut pair,
            &plan,
            &base_kernel,
            &base_sources,
            step,
            FieldStepOptions {
                source_kernels: Some(&[base_kernel]),
                flux_edges: None,
            },
        ),
        Err(CoreError::LayoutMismatch { .. })
    ));
    let mixed = vec![base_kernel, wrong_mode, base_kernel, base_kernel];
    assert!(matches!(
        device.advance_v2(
            &mut pair,
            &plan,
            &base_kernel,
            &base_sources,
            step,
            FieldStepOptions {
                source_kernels: Some(&mixed),
                flux_edges: None,
            },
        ),
        Err(CoreError::StateConflict { .. })
    ));
}

/// `V2-09` (`tight-junctions`): 16 ticks on a `64×64` heterogeneous occupancy
/// grid (both `HarmonicV1` and `MinV1`, 1-substep and multi-substep, and
/// combined with `V2-02` pulsing heterogeneous oases) plus parameterized
/// `plane_range` slices (`MetalField::transport_flux_range`) match CPU
/// bit-for-bit on every F32 value and report field.
#[test]
fn metal_field_flux_stencil_bit_exact_with_cpu() {
    let Some(mut device) = metal() else {
        return;
    };
    let mut rng = Rng(0x2026_0909);
    let grid = FieldGrid::new(64, 64, 64.0, 64.0).expect("grid");
    let step = TimeStep::new(1, 32).expect("step");

    // Build a dense 1-cell ring + scattered clusters to exercise sharp
    // permeability jumps across both axes and torus seams.
    let mut positions = Vec::new();
    for x in 16..48 {
        for _ in 0..5 {
            positions.push([f64::from(x) + 0.5, 16.5]);
            positions.push([f64::from(x) + 0.5, 47.5]);
        }
    }
    for y in 17..47 {
        for _ in 0..4 {
            positions.push([16.5, f64::from(y) + 0.5]);
            positions.push([47.5, f64::from(y) + 0.5]);
        }
    }
    // Seam cells at x=0 and y=63.
    for _ in 0..3 {
        positions.push([0.25, 63.75]);
        positions.push([63.75, 0.25]);
    }

    let centers = [[20.5, 20.5], [40.25, 40.75], [5.5, 58.25], [58.5, 5.25]];
    let mut sources: Vec<Source> = centers
        .iter()
        .enumerate()
        .map(|(index, &center)| {
            let mut direction = [0.0; 32];
            for (c, slot) in direction.iter_mut().enumerate() {
                if (c + index) % 2 == 0 {
                    *slot = (rng.unit() - 0.5) * 0.25;
                }
            }
            Source::new(SourceId::new(index as u32), center, direction, 95.0).expect("source")
        })
        .collect();
    let schedule = OasisSchedule::new(1.5, 0.0, 1.8, 3.5).expect("oasis");
    let seed_rng = SemanticRng::new(0x0909_2026);
    let base_kernel = SourceKernel::new_fused(2.5, 4.0).expect("fused");
    let kernels = schedule
        .seed_kernels(&seed_rng, &base_kernel, &sources)
        .expect("kernels");

    for (mode, diffusion, decay) in [
        (BarrierMode::HarmonicV1, 0.35, 0.02),
        (BarrierMode::MinV1, 28.0, 0.0), // multi-substep (>1) + zero decay
    ] {
        let plan = TransportPlan::new(&grid, step, diffusion, decay).expect("plan");
        let barrier = TightJunctions::new(mode, 0.04, 1.2).expect("barrier");
        let mut work =
            FieldWork::new(&grid, &LIMITS, NonZeroUsize::new(4).expect("4")).expect("work");
        work.reserve_supports_for_kernels(&kernels)
            .expect("reserve supports");
        work.reserve_flux().expect("reserve flux");
        let edges = work
            .prepare_tight_junctions(&barrier, &plan, &positions)
            .expect("prepare edges")
            .clone();

        let mut reference = random_pair(grid, &mut rng);
        let mut accelerated = FieldPair::zeroed(grid, &LIMITS).expect("pair");
        accelerated
            .current_mut()
            .copy_from(reference.current())
            .expect("copy");
        let mut power_scratch = Vec::new();
        for tick in 0..16_u64 {
            schedule
                .apply_powers(
                    Tick::new(tick),
                    step,
                    95.0,
                    &mut sources,
                    &mut power_scratch,
                )
                .expect("apply powers");
            let options = FieldStepOptions {
                source_kernels: Some(&kernels),
                flux_edges: Some(&edges),
            };
            let expected = advance_field_v2_on(
                &mut reference,
                &plan,
                &base_kernel,
                &sources,
                step,
                options,
                &mut work,
            )
            .expect("cpu flux step");
            let got = device
                .advance_v2(
                    &mut accelerated,
                    &plan,
                    &base_kernel,
                    &sources,
                    step,
                    options,
                )
                .expect("metal flux step");
            let (want, have) = (
                reference.stage().1.as_slice(),
                accelerated.stage().1.as_slice(),
            );
            let differing = want
                .iter()
                .zip(have)
                .filter(|(a, b)| a.to_bits() != b.to_bits())
                .count();
            assert_eq!(
                differing,
                0,
                "mode={mode:?} substeps={} tick={tick}: {differing} values differ",
                plan.substeps()
            );
            assert_eq!(
                report_bits(&expected),
                report_bits(&got),
                "mode={mode:?} tick={tick}: reports differ"
            );
            reference.commit();
            accelerated.commit();
        }

        // Parameterized `plane_range` parity (reused by V2-14 & V2-15) including
        // non-zero `first_plane` and verifying untouched planes stay bit-identical.
        let src_pair = random_pair(grid, &mut rng);
        let sentinel_pair = random_pair(grid, &mut rng);
        let mut scratches = vec![
            TransportScratch::new(&grid, &LIMITS).expect("scratch"),
            TransportScratch::new(&grid, &LIMITS).expect("scratch"),
        ];
        for (plane_range, energy_plane) in [
            (0..1, Some(0)),
            (1..17, None),
            (17..33, Some(0)),
            (0..FIELD_PLANES as usize, Some(0)),
        ] {
            let mut cpu_next = FieldState::zeroed(grid, &LIMITS).expect("state");
            let mut gpu_next = FieldState::zeroed(grid, &LIMITS).expect("state");
            cpu_next.copy_from(sentinel_pair.current()).expect("copy");
            gpu_next.copy_from(sentinel_pair.current()).expect("copy");

            let want_report = transport_flux_range_on(
                &plan,
                &edges,
                src_pair.current(),
                &mut cpu_next,
                plane_range.clone(),
                energy_plane,
                &mut scratches,
            )
            .expect("cpu flux range");
            let got_report = device
                .transport_flux_range(
                    &plan,
                    &edges,
                    src_pair.current(),
                    &mut gpu_next,
                    plane_range.clone(),
                    energy_plane,
                )
                .expect("metal flux range");

            assert_eq!(
                cpu_next
                    .as_slice()
                    .iter()
                    .map(|v| v.to_bits())
                    .collect::<Vec<_>>(),
                gpu_next
                    .as_slice()
                    .iter()
                    .map(|v| v.to_bits())
                    .collect::<Vec<_>>(),
                "mode={mode:?} plane_range={plane_range:?} mismatch"
            );
            assert_eq!(want_report.is_some(), got_report.is_some());
            if let (Some(w), Some(g)) = (want_report, got_report) {
                assert_eq!(w.substeps, g.substeps);
                assert_eq!(w.energy_before.to_bits(), g.energy_before.to_bits());
                assert_eq!(
                    w.energy_transported.to_bits(),
                    g.energy_transported.to_bits()
                );
                assert_eq!(w.energy_after.to_bits(), g.energy_after.to_bits());
                assert_eq!(w.decay_removed.to_bits(), g.decay_removed.to_bits());
                assert_eq!(
                    w.transport_residual.to_bits(),
                    g.transport_residual.to_bits()
                );
            }
        }
    }

    // Negative refusals on invalid FluxEdges (wrong grid, out-of-range conductance).
    let plan = TransportPlan::new(&grid, step, 0.25, 0.02).expect("plan");
    let (r_x, _) = plan.ratios();
    let other_grid = FieldGrid::new(32, 32, 32.0, 32.0).expect("other grid");
    let wrong_edges = FluxEdges::new(&other_grid, &LIMITS).expect("wrong edges");
    let mut pair = random_pair(grid, &mut rng);
    assert!(matches!(
        device.advance_v2(
            &mut pair,
            &plan,
            &base_kernel,
            &sources,
            step,
            FieldStepOptions {
                source_kernels: None,
                flux_edges: Some(&wrong_edges),
            },
        ),
        Err(CoreError::LayoutMismatch { .. })
    ));
    for bad in [-1.0e-9, r_x * 1.0001, f64::NAN, f64::INFINITY] {
        let mut bad_edges = FluxEdges::new(&grid, &LIMITS).expect("edges");
        bad_edges.edge_x_mut()[17] = bad;
        assert!(matches!(
            device.advance_v2(
                &mut pair,
                &plan,
                &base_kernel,
                &sources,
                step,
                FieldStepOptions {
                    source_kernels: None,
                    flux_edges: Some(&bad_edges),
                },
            ),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }
}

/// `V2-14` (`vascular-field`): 32 ticks on a `64×64` heterogeneous grid for
/// BOTH `μ = 1.5` (fast path `r * sqrt(r)`) and `μ = 0.7` (Brenner sublinear
/// control via hoisted `d_pow_positive`), composed multiplicatively with
/// `V2-02` pulsing heterogeneous oases and `V2-09` tight junctions (`HarmonicV1`
/// and `MinV1`, multi-substep `D_max`), plus C.8 cancel-safety, extreme flux
/// overflow guard (`ratio > 1e100 -> sat = 1.0`), zero-flux pruning, 0 B lazy
/// VRAM when disabled, and negative refusals.
#[test]
fn metal_vascular_field_bit_exact_with_cpu_mu_1_5_and_0_7() {
    use flife_field::{FieldStepPhase2, VascularField, advance_field_wave2_on};

    let Some(mut device) = metal() else {
        return;
    };
    let mut rng = Rng(0x2026_1414);
    let grid = FieldGrid::new(64, 64, 64.0, 64.0).expect("grid");
    let step = TimeStep::new(1, 32).expect("step");

    // 0 B extra VRAM when vascular/flux are disabled: fresh MetalField running
    // baseline v1 advance allocates neither flux nor vascular buffers, and grows
    // only once vascular is first enabled.
    let baseline_plan = TransportPlan::new(&grid, step, 0.25, 0.02).expect("baseline plan");
    let baseline_kernel = SourceKernel::new_fused(2.5, 4.0).expect("baseline kernel");
    let mut sources: Vec<Source> = [[12.5, 14.25], [50.25, 16.5], [18.0, 50.75], [52.5, 48.25]]
        .iter()
        .enumerate()
        .map(|(index, &center)| {
            let mut direction = [0.0; 32];
            for (c, slot) in direction.iter_mut().enumerate() {
                if (c + index) % 2 == 0 {
                    *slot = (rng.unit() - 0.5) * 0.25;
                }
            }
            Source::new(SourceId::new(index as u32), center, direction, 110.0).expect("source")
        })
        .collect();
    {
        let mut fresh = MetalField::open().expect("fresh metal field");
        let mut pair = random_pair(grid, &mut rng);
        fresh
            .advance(&mut pair, &baseline_plan, &baseline_kernel, &sources, step)
            .expect("v1 advance 1");
        let v1_bytes = fresh.owner().usage().bytes;
        fresh
            .advance(&mut pair, &baseline_plan, &baseline_kernel, &sources, step)
            .expect("v1 advance 2");
        assert_eq!(
            fresh.owner().usage().bytes,
            v1_bytes,
            "repeated v1 advance must allocate 0 extra VRAM"
        );

        let vascular = VascularField::new(0.05, 2.5, 0.8, 0.15, 0.05, 1.5).expect("vascular");
        let vasc_plan = vascular
            .transport_plan(&grid, step, 0.02)
            .expect("vasc plan");
        pair.enable_vascular(vascular.initial_conductance(), &LIMITS)
            .expect("enable vascular");
        let mut edges = FluxEdges::new(&grid, &LIMITS).expect("edges");
        let (px, py) = pair.current().vascular_edges().expect("edges");
        vascular
            .fill_edges(&vasc_plan, px, py, None, &mut edges)
            .expect("fill edges");
        fresh
            .advance_wave2(
                &mut pair,
                &vasc_plan,
                &baseline_kernel,
                &sources,
                step,
                FieldStepPhase2 {
                    source_kernels: None,
                    carrier0_edges: Some(&edges),
                    vascular: Some(&vascular),
                    trail_plan: None,
                    trail_edges: None,
                },
            )
            .expect("first vascular advance");
        let vasc_bytes = fresh.owner().usage().bytes;
        assert!(
            vasc_bytes > v1_bytes,
            "lazy flux + vascular buffers should allocate only on first vascular use"
        );
        fresh
            .advance_wave2(
                &mut pair,
                &vasc_plan,
                &baseline_kernel,
                &sources,
                step,
                FieldStepPhase2 {
                    source_kernels: None,
                    carrier0_edges: Some(&edges),
                    vascular: Some(&vascular),
                    trail_plan: None,
                    trail_edges: None,
                },
            )
            .expect("second vascular advance");
        assert_eq!(
            fresh.owner().usage().bytes,
            vasc_bytes,
            "steady-state vascular advance must allocate 0 extra VRAM"
        );
    }

    // Heterogeneous organism cluster positions for multiplicative V2-09 + V2-14 composition.
    let mut positions = Vec::new();
    for x in 18..46 {
        for _ in 0..4 {
            positions.push([f64::from(x) + 0.5, 18.5]);
            positions.push([f64::from(x) + 0.5, 45.5]);
        }
    }
    for y in 19..45 {
        for _ in 0..3 {
            positions.push([18.5, f64::from(y) + 0.5]);
            positions.push([45.5, f64::from(y) + 0.5]);
        }
    }
    let schedule = OasisSchedule::new(1.2, 0.0, 1.8, 3.8).expect("oasis");
    let seed_rng = SemanticRng::new(0x1414_2026);
    let kernels = schedule
        .seed_kernels(&seed_rng, &baseline_kernel, &sources)
        .expect("kernels");

    for (mu, mode, d_max) in [
        (1.5_f64, BarrierMode::HarmonicV1, 2.5_f64),
        (0.7_f64, BarrierMode::MinV1, 24.0_f64), // multi-substep + general pow_positive
    ] {
        let vascular = VascularField::new(0.05, d_max, 0.9, 0.18, 0.04, mu).expect("vascular");
        let plan = vascular.transport_plan(&grid, step, 0.02).expect("plan");
        let barrier = TightJunctions::new(mode, 0.05, 1.1).expect("barrier");
        let mut work =
            FieldWork::new(&grid, &LIMITS, NonZeroUsize::new(4).expect("4")).expect("work");
        work.reserve_supports_for_kernels(&kernels)
            .expect("reserve supports");
        work.reserve_flux().expect("reserve flux");

        let mut reference = random_pair(grid, &mut rng);
        reference
            .enable_vascular(vascular.initial_conductance(), &LIMITS)
            .expect("ref vascular");
        let mut accelerated = FieldPair::zeroed(grid, &LIMITS).expect("pair");
        accelerated
            .enable_vascular(vascular.initial_conductance(), &LIMITS)
            .expect("gpu vascular");
        accelerated
            .current_mut()
            .copy_from(reference.current())
            .expect("copy");

        let mut power_scratch = Vec::new();
        for tick in 0..32_u64 {
            schedule
                .apply_powers(
                    Tick::new(tick),
                    step,
                    110.0,
                    &mut sources,
                    &mut power_scratch,
                )
                .expect("apply powers");
            let (px_before, py_before) = {
                let (px, py) = accelerated.current().vascular_edges().expect("committed");
                (px.to_vec(), py.to_vec())
            };
            let edges = work
                .prepare_vascular_edges(
                    &vascular,
                    &plan,
                    &px_before,
                    &py_before,
                    Some((&barrier, &positions)),
                )
                .expect("prepare vascular edges")
                .clone();

            let phase2 = FieldStepPhase2 {
                source_kernels: Some(&kernels),
                carrier0_edges: Some(&edges),
                vascular: Some(&vascular),
                trail_plan: None,
                trail_edges: None,
            };
            let expected = advance_field_wave2_on(
                &mut reference,
                &plan,
                &baseline_kernel,
                &sources,
                step,
                phase2,
                &mut work,
            )
            .expect("cpu vascular step");
            let got = device
                .advance_wave2(
                    &mut accelerated,
                    &plan,
                    &baseline_kernel,
                    &sources,
                    step,
                    phase2,
                )
                .expect("metal vascular step");

            // C.8 cancel-safety: committed vascular edges are untouched until commit().
            let (px_still, py_still) = accelerated.current().vascular_edges().expect("committed");
            assert_eq!(
                px_still.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                px_before.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                "mu={mu} tick={tick}: committed edge_x mutated before commit"
            );
            assert_eq!(
                py_still.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                py_before.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                "mu={mu} tick={tick}: committed edge_y mutated before commit"
            );

            // Bit-exact field planes, reports, and candidate vascular edges.
            let (want, have) = (
                reference.stage().1.as_slice(),
                accelerated.stage().1.as_slice(),
            );
            let differing = want
                .iter()
                .zip(have)
                .filter(|(a, b)| a.to_bits() != b.to_bits())
                .count();
            assert_eq!(
                differing, 0,
                "mu={mu} mode={mode:?} tick={tick}: {differing} field values differ"
            );
            assert_eq!(
                report_bits(&expected),
                report_bits(&got),
                "mu={mu} mode={mode:?} tick={tick}: reports differ"
            );
            let (want_x, want_y) = reference.next().vascular_edges().expect("ref next edges");
            let (got_x, got_y) = accelerated.next().vascular_edges().expect("gpu next edges");
            let diff_x = want_x
                .iter()
                .zip(got_x)
                .filter(|(a, b)| a.to_bits() != b.to_bits())
                .count();
            let diff_y = want_y
                .iter()
                .zip(got_y)
                .filter(|(a, b)| a.to_bits() != b.to_bits())
                .count();
            assert_eq!(
                diff_x + diff_y,
                0,
                "mu={mu} mode={mode:?} tick={tick}: vascular edges differ (x={diff_x}, y={diff_y})"
            );

            reference.commit();
            accelerated.commit();
        }

        // Verify conductances actually evolved away from initial value.
        let (final_x, _) = accelerated.current().vascular_edges().expect("final edges");
        assert!(
            final_x
                .iter()
                .any(|&g| g.to_bits() != vascular.initial_conductance().to_bits()),
            "mu={mu}: expected vascular conductances to evolve"
        );
    }

    // Overflow guard (`ratio > 1e100 -> sat = 1.0`) and zero-flux pure pruning
    // for both μ = 1.5 and μ = 2.4 (where ratio^mu would overflow without guard).
    for mu in [1.5_f64, 2.4_f64] {
        let small = FieldGrid::new(8, 8, 8.0, 8.0).expect("small grid");
        let vascular = VascularField::new(0.05, 1.0, 0.8, 0.2, 1.0e-12, mu).expect("vascular");
        let plan = vascular.transport_plan(&small, step, 0.0).expect("plan");
        let mut work =
            FieldWork::new(&small, &LIMITS, NonZeroUsize::new(2).expect("2")).expect("work");
        let mut reference = FieldPair::zeroed(small, &LIMITS).expect("pair");
        // Half the grid has extreme jump (e = 1e35 vs 0) -> ratio >> 1e40 (overflows at mu=2.4)
        // and cell 0 has ratio > 1e100 via tiny J_0; other cells are uniform (zero flux).
        reference.current_mut().energy_mut().fill(5.0);
        reference.current_mut().energy_mut()[0] = 1.0e35_f32;
        reference.current_mut().energy_mut()[1] = 0.0_f32;
        reference
            .enable_vascular(0.5, &LIMITS)
            .expect("enable vascular");
        let mut accelerated = reference.clone();
        let mut edges = FluxEdges::new(&small, &LIMITS).expect("edges");
        let (px, py) = reference.current().vascular_edges().expect("edges");
        vascular
            .fill_edges(&plan, px, py, None, &mut edges)
            .expect("fill");
        let phase2 = FieldStepPhase2 {
            source_kernels: None,
            carrier0_edges: Some(&edges),
            vascular: Some(&vascular),
            trail_plan: None,
            trail_edges: None,
        };
        let expected = advance_field_wave2_on(
            &mut reference,
            &plan,
            &baseline_kernel,
            &[],
            step,
            phase2,
            &mut work,
        )
        .expect("cpu extreme vascular");
        let got = device
            .advance_wave2(&mut accelerated, &plan, &baseline_kernel, &[], step, phase2)
            .expect("metal extreme vascular");
        assert_eq!(report_bits(&expected), report_bits(&got));
        let (wx, wy) = reference.next().vascular_edges().expect("ref");
        let (gx, gy) = accelerated.next().vascular_edges().expect("gpu");
        assert_eq!(
            wx.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            gx.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            "mu={mu} overflow/zero-flux edge_x mismatch"
        );
        assert_eq!(
            wy.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            gy.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            "mu={mu} overflow/zero-flux edge_y mismatch"
        );
        // Zero-flux interior edge strictly pruned below 0.5; extreme edge reinforced above 0.5.
        assert!(gx[0] > 0.5, "mu={mu}: extreme flux edge must reinforce");
        assert!(gx[27] < 0.5, "mu={mu}: zero flux edge must prune");
    }

    // Negative refusals: vascular without carrier0_edges (`StateConflict`) and
    // vascular without enabled FieldPair buffers (`LayoutMismatch`).
    let vascular = VascularField::new(0.05, 2.5, 0.8, 0.15, 0.05, 1.5).expect("vascular");
    let vasc_plan = vascular.transport_plan(&grid, step, 0.02).expect("plan");
    let mut no_vasc_pair = random_pair(grid, &mut rng);
    let mut dummy_edges = FluxEdges::new(&grid, &LIMITS).expect("edges");
    dummy_edges.edge_x_mut().fill(vasc_plan.ratios().0 * 0.5);
    dummy_edges.edge_y_mut().fill(vasc_plan.ratios().1 * 0.5);
    assert!(matches!(
        device.advance_wave2(
            &mut no_vasc_pair,
            &vasc_plan,
            &baseline_kernel,
            &sources,
            step,
            FieldStepPhase2 {
                source_kernels: None,
                carrier0_edges: Some(&dummy_edges),
                vascular: Some(&vascular),
                trail_plan: None,
                trail_edges: None,
            },
        ),
        Err(CoreError::LayoutMismatch { .. })
    ));
    no_vasc_pair
        .enable_vascular(vascular.initial_conductance(), &LIMITS)
        .expect("enable");
    assert!(matches!(
        device.advance_wave2(
            &mut no_vasc_pair,
            &vasc_plan,
            &baseline_kernel,
            &sources,
            step,
            FieldStepPhase2 {
                source_kernels: None,
                carrier0_edges: None,
                vascular: Some(&vascular),
                trail_plan: None,
                trail_edges: None,
            },
        ),
        Err(CoreError::StateConflict { .. })
    ));
}

/// `V2-15` (`34-plane dual-carrier-trails`): bit-exact CPU parity across ALL 6
/// Metal kernels on a `64×64` 34-plane field:
/// 1. `field.metal` (`advance_wave2` with V2-02 + V2-09 on both carriers + V2-14 on Carrier 0 only + V2-15);
/// 2. `sensors.metal` (`MetalSensors::sample_dual` feeding `observe_dual_for_controller`);
/// 3. `chemistry.metal` (`MetalChemistry::settle` ignoring Carrier-1 planes 25..33);
/// 4. `reduce.metal` (`grants_debit` leaving Carrier-1 bit-for-bit untouched across 50 feeding organisms + `reduce_dual_carrier`);
/// 5. `check.metal` (`MetalField::check` catching Carrier-0 and Carrier-1-only cone & negative `e_1` faults);
/// 6. `controller.metal` (`MetalBackend` F16 & F32 on 108-input Bundle B `108 -> [64, 32] -> 42` with 0 alignment deferrals).
///
/// Also checks negative refusals for 33/34-plane mismatches.
#[test]
fn metal_dual_carrier_34_planes_all_6_kernels_bit_exact_with_cpu() {
    use flife_backend_metal::{
        ChemistryConstants, ChemistryOrganism, ChemistryPacket, MetalBackend, MetalChemistry,
        MetalSensors,
    };
    use flife_core::binary16::F16;
    use flife_core::ids::{GenomeId, OrganismId};
    use flife_core::ledger::EnergyBudget;
    use flife_engine::{Affinity, ChemicalPacket, Chemistry, composition};
    use flife_field::grid::{
        CARRIER0_MOMENT_CHANNELS, CARRIER1_ENERGY_PLANE, CARRIER1_MOMENT_CHANNELS,
        DUAL_CARRIER_PLANES,
    };
    use flife_field::{
        ConeTolerance, Demand, DepositSummary, Deposits, DualCarrierRules, FieldStepPhase2, Grants,
        SensorBackend, SensorPyramid, SensorQuery, VascularField, advance_field_wave2_on,
        reduce_dual_carrier_field_on, resolve_uptake, split_dual_carrier_emission,
    };
    use flife_neural::{
        ArenaSlot, ControllerBackend, ControllerJob, ControllerScratch, DenseTopology, Intrinsic,
        ReferenceController, SensorScales, observe_dual_for_controller,
    };

    let Some(mut field_dev) = metal() else {
        return;
    };
    let mut sensors_dev = MetalSensors::with_owner(field_dev.owner().clone()).expect("sensors");
    let mut chem_dev = MetalChemistry::with_owner(field_dev.owner().clone()).expect("chemistry");
    let mut ctrl_f32 = MetalBackend::open().expect("ctrl f32");
    let mut ctrl_f16 = MetalBackend::open().expect("ctrl f16");

    let mut rng = Rng(0x2026_1515);
    let grid = FieldGrid::new(64, 64, 64.0, 64.0).expect("grid");
    let step = TimeStep::new(1, 32).expect("step");

    // Construct a valid 34-plane initial FieldPair with both carriers populated
    // strictly inside their respective cones (||m^(0)|| <= e_0, ||m^(1)|| <= e_1).
    let mut reference =
        FieldPair::zeroed_with_planes(grid, DUAL_CARRIER_PLANES, &LIMITS).expect("34-plane pair");
    {
        let cur = reference.current_mut();
        let cells = grid.cell_count() as usize;
        for c in 0..cells {
            let e0 = (0.5 + rng.unit() * 6.0) as f32;
            cur.energy_mut()[c] = e0;
            let scale0 = f64::from(e0) * 0.08;
            for ch in 0..CARRIER0_MOMENT_CHANNELS {
                cur.moment_mut(ch).expect("c0")[c] = ((rng.unit() * 2.0 - 1.0) * scale0) as f32;
            }
            let e1 = (0.2 + rng.unit() * 3.0) as f32;
            cur.trail_energy_mut().expect("e1")[c] = e1;
            let scale1 = f64::from(e1) * 0.12;
            for ch in 0..CARRIER1_MOMENT_CHANNELS {
                cur.trail_moment_mut(ch).expect("c1")[c] =
                    ((rng.unit() * 2.0 - 1.0) * scale1) as f32;
            }
        }
    }

    // -------------------------------------------------------------------------
    // 1. field.metal: 16 ticks composing V2-02 + V2-09 (C0 & C1) + V2-14 (C0 only) + V2-15.
    // -------------------------------------------------------------------------
    let vascular = VascularField::new(0.08, 6.0, 0.85, 0.15, 0.04, 1.5).expect("vascular");
    let c0_plan = vascular
        .transport_plan(&grid, step, 0.025)
        .expect("c0 plan");
    let trail_rules = DualCarrierRules::new(0.06, 0.002, 0.25, 0.5).expect("trail rules");
    let trail_plan = trail_rules.trail_plan(&grid, step).expect("trail plan");
    let barrier = TightJunctions::new(BarrierMode::HarmonicV1, 0.05, 1.0).expect("barrier");

    let mut positions = Vec::with_capacity(64);
    for i in 0..64 {
        positions.push([((i % 8) as f64) * 7.5 + 3.25, ((i / 8) as f64) * 7.5 + 4.75]);
    }

    let mut sources: Vec<Source> = [[14.5, 18.25], [48.75, 14.5], [16.25, 50.0], [50.5, 48.75]]
        .iter()
        .enumerate()
        .map(|(index, &center)| {
            let mut direction = [0.0; 32];
            for slot in direction.iter_mut().take(CARRIER0_MOMENT_CHANNELS as usize) {
                *slot = (rng.unit() - 0.5) * 0.22;
            }
            // Deliberately put nonzero direction components on 24..32 to prove
            // neither CPU nor Metal deposits sources onto Carrier-1 planes.
            direction[24 + (index % 8)] = 0.05;
            Source::new(SourceId::new(index as u32), center, direction, 90.0).expect("source")
        })
        .collect();
    let schedule = OasisSchedule::new(1.2, 0.0, 1.6, 3.2).expect("oasis");
    let seed_rng = SemanticRng::new(0x1515_2026);
    let base_kernel = SourceKernel::new_fused(2.5, 4.0).expect("fused");
    let kernels = schedule
        .seed_kernels(&seed_rng, &base_kernel, &sources)
        .expect("kernels");

    reference
        .enable_vascular(vascular.initial_conductance(), &LIMITS)
        .expect("enable vasc");
    let mut accelerated = reference.clone();
    let mut work = FieldWork::new(&grid, &LIMITS, NonZeroUsize::new(4).expect("4")).expect("work");
    work.reserve_supports_for_kernels(&kernels)
        .expect("reserve supports");
    work.reserve_flux().expect("reserve flux");
    work.reserve_trail_flux().expect("reserve trail flux");

    let mut power_scratch = Vec::new();
    for tick in 0..16_u64 {
        schedule
            .apply_powers(
                Tick::new(tick),
                step,
                90.0,
                &mut sources,
                &mut power_scratch,
            )
            .expect("apply powers");
        let (px, py) = {
            let (x, y) = accelerated.current().vascular_edges().expect("vasc edges");
            (x.to_vec(), y.to_vec())
        };
        let c0_edges = work
            .prepare_vascular_edges(&vascular, &c0_plan, &px, &py, Some((&barrier, &positions)))
            .expect("c0 edges")
            .clone();
        // Carrier-1 gets tight junctions only (never vascular arteries).
        let mut c1_edges = FluxEdges::new(&grid, &LIMITS).expect("c1 edges");
        barrier
            .fill_edges(&trail_plan, work.cell_occupancy(), &mut c1_edges)
            .expect("c1 fill edges");

        let phase2 = FieldStepPhase2 {
            source_kernels: Some(&kernels),
            carrier0_edges: Some(&c0_edges),
            vascular: Some(&vascular),
            trail_plan: Some(&trail_plan),
            trail_edges: Some(&c1_edges),
        };
        let expected = advance_field_wave2_on(
            &mut reference,
            &c0_plan,
            &base_kernel,
            &sources,
            step,
            phase2,
            &mut work,
        )
        .expect("cpu dual-carrier step");
        let got = field_dev
            .advance_wave2(
                &mut accelerated,
                &c0_plan,
                &base_kernel,
                &sources,
                step,
                phase2,
            )
            .expect("metal dual-carrier step");

        let (want, have) = (
            reference.stage().1.as_slice(),
            accelerated.stage().1.as_slice(),
        );
        let differing = want
            .iter()
            .zip(have)
            .filter(|(a, b)| a.to_bits() != b.to_bits())
            .count();
        assert_eq!(
            differing, 0,
            "dual-carrier tick={tick}: {differing} F32 values differ"
        );
        assert_eq!(
            report_bits(&expected),
            report_bits(&got),
            "dual-carrier tick={tick}: reports differ"
        );
        reference.commit();
        accelerated.commit();
    }

    // -------------------------------------------------------------------------
    // 2. sensors.metal: 256 dual-carrier queries bit-exact vs PyramidView::sample_dual
    //    + observe_dual_for_controller feeding 108-input Bundle B controllers.
    // -------------------------------------------------------------------------
    let mut pyramid =
        SensorPyramid::new_with_planes(grid, 4, DUAL_CARRIER_PLANES as usize, &LIMITS)
            .expect("pyramid");
    let queries: Vec<SensorQuery> = (0..256)
        .map(|i| SensorQuery {
            position: [rng.unit() * 64.0, rng.unit() * 64.0],
            scale: 1.0 + (i % 7) as f64 * 0.85,
        })
        .collect();
    let mut gpu_samples = Vec::new();
    sensors_dev
        .sample_dual(&pyramid, accelerated.current(), &queries, &mut gpu_samples)
        .expect("metal sample_dual");
    assert_eq!(gpu_samples.len(), queries.len());
    let view = pyramid.build(accelerated.current()).expect("build view");

    let scales = SensorScales::new([1.0, 4.0, 0.05, 10.0, 100.0, 2.0, 5.0], 0.5, 16.0)
        .expect("sensor scales");
    let intrinsic = Intrinsic {
        energy: 4.0,
        age_seconds: 12.0,
        velocity: [0.3, -0.2],
        energy_change_per_second: 0.1,
    };
    let mut sensor_108: Vec<Vec<f64>> = Vec::with_capacity(64);
    for (idx, (query, got_sample)) in queries.iter().zip(&gpu_samples).enumerate() {
        let want_sample = view
            .sample_dual(query.position, query.scale)
            .expect("cpu sample_dual");
        for p in 0..DUAL_CARRIER_PLANES as usize {
            assert_eq!(
                want_sample.values[p].to_bits(),
                got_sample.values[p].to_bits(),
                "sensor query={idx} plane={p} value differs"
            );
            assert_eq!(
                want_sample.dx[p].to_bits(),
                got_sample.dx[p].to_bits(),
                "sensor query={idx} plane={p} dx differs"
            );
            assert_eq!(
                want_sample.dy[p].to_bits(),
                got_sample.dy[p].to_bits(),
                "sensor query={idx} plane={p} dy differs"
            );
        }
        if idx < 64 {
            let dual_obs = observe_dual_for_controller(
                got_sample,
                &intrinsic,
                &scales,
                trail_rules.trail_energy_density_scale(),
            )
            .expect("observe_dual_for_controller");
            let mut input = Vec::with_capacity(108);
            input.extend_from_slice(&dual_obs.base.inputs);
            input.extend_from_slice(&dual_obs.trail_inputs);
            assert_eq!(input.len(), 108);
            sensor_108.push(input);
        }
    }

    // -------------------------------------------------------------------------
    // 3. chemistry.metal: 64 organisms with nonzero affinity[24..32] on a
    //    34-plane field whose Carrier-1 holds huge e_1 = 1000.0 & nonzero m^(1).
    // -------------------------------------------------------------------------
    let mut chem_field = accelerated.current().clone();
    chem_field.trail_energy_mut().expect("e1").fill(1000.0);
    for c in 0..CARRIER1_MOMENT_CHANNELS {
        chem_field
            .trail_moment_mut(c)
            .expect("c1")
            .fill(if c % 2 == 0 { 500.0 } else { -500.0 });
    }
    let chem_cpu = Chemistry::new(0.85, 3.0, 0.25, 1.5, 1.0).expect("chemistry");
    let chem_consts = ChemistryConstants {
        maximum_efficiency: 0.85,
        gain: 3.0,
        threshold: 0.25,
        toxicity_sensitivity: 1.5,
        affinity_norm_limit: 1.0,
    };
    let cells = grid.cell_count() as usize;
    let mut chem_orgs = Vec::with_capacity(64);
    let mut chem_packets = Vec::new();
    let mut chem_want = Vec::with_capacity(64);
    for i in 0..64 {
        let mut aff = [0.0_f64; 32];
        for (c, slot) in aff.iter_mut().enumerate() {
            *slot = ((rng.unit() * 2.0 - 1.0) * 0.14) + if c >= 24 { 0.03 } else { 0.0 };
        }
        let norm = aff.iter().map(|v| v * v).sum::<f64>().sqrt();
        if norm > 0.9 {
            for slot in &mut aff {
                *slot *= 0.9 / norm;
            }
        }
        let remaining = 0.4 + rng.unit() * 4.0;
        let first = chem_packets.len();
        let count = 1 + (i % 4);
        let mut cpu_pkts = Vec::with_capacity(count);
        for k in 0..count {
            let cell = (i * 53 + k * 19) % cells;
            let granted = 0.02 + rng.unit() * 0.4;
            chem_packets.push(ChemistryPacket { cell, granted });
            let z = composition(&chem_field, cell).expect("cpu composition");
            assert_eq!(
                &z[CARRIER0_MOMENT_CHANNELS as usize..],
                &[0.0_f64; 8],
                "CPU composition must zero channels 24..32 on 34-plane field"
            );
            cpu_pkts.push(ChemicalPacket {
                granted,
                composition: z,
            });
        }
        let affinity = Affinity::new(aff, &chem_cpu).expect("affinity");
        let mut budget = EnergyBudget::new(remaining).expect("budget");
        let settlement = chem_cpu
            .settle(&affinity, &cpu_pkts, &mut budget)
            .expect("cpu settle");
        chem_want.push((
            settlement.absorbed.to_bits(),
            settlement.gain.to_bits(),
            settlement.toxicity.requested.to_bits(),
            settlement.toxicity.paid.to_bits(),
            budget.remaining().to_bits(),
            budget.is_exhausted(),
        ));
        chem_orgs.push(ChemistryOrganism {
            affinity: aff,
            remaining,
            exhausted: false,
            first,
            packets: count,
        });
    }
    let mut chem_got = Vec::new();
    chem_dev
        .settle(
            &chem_field,
            &chem_consts,
            &chem_orgs,
            &chem_packets,
            &mut chem_got,
        )
        .expect("metal chemistry settle");
    assert_eq!(chem_got.len(), chem_want.len());
    for (i, (got, want)) in chem_got.iter().zip(&chem_want).enumerate() {
        let got_bits = (
            got.absorbed.to_bits(),
            got.gain.to_bits(),
            got.requested_toxicity.to_bits(),
            got.paid_toxicity.to_bits(),
            got.remaining.to_bits(),
            got.exhausted,
        );
        assert_eq!(got_bits, *want, "chemistry 34-plane org={i} differs");
    }

    // -------------------------------------------------------------------------
    // 4. reduce.metal:
    //    (a) 50 feeding organisms' grants_debit leaves Carrier-1 (planes 25..34)
    //        BIT-FOR-BIT UNTOUCHED;
    //    (b) full reduce_dual_carrier (grants + emissions_0 + recycling + trail)
    //        matches CPU reduce_dual_carrier_field_on bit-for-bit on all 34 planes
    //        and all 3 DepositSummary structs.
    // -------------------------------------------------------------------------
    let feeding_demands: Vec<Demand> = (0..50)
        .map(|i| Demand {
            organism: OrganismId::new(100 + i as u64 * 3),
            position: [
                ((i % 10) as f64) * 6.1 + 1.4,
                ((i / 10) as f64) * 11.3 + 2.7,
            ],
            amount: 0.35 + (i % 5) as f64 * 0.2,
        })
        .collect();
    {
        let mut debit_only_field = accelerated.current().clone();
        let c1_start = (CARRIER1_ENERGY_PLANE as usize) * cells;
        let c1_before_bits: Vec<u32> = debit_only_field.as_slice()[c1_start..]
            .iter()
            .map(|v| v.to_bits())
            .collect();
        let mut grants = Grants::new();
        resolve_uptake(&debit_only_field, &feeding_demands, &mut grants).expect("resolve uptake");
        assert!(
            grants.summary().granted > 0.0,
            "expected positive uptake across 50 feeding organisms"
        );
        let (mut e0, mut rec, mut tr) = (Deposits::new(), Deposits::new(), Deposits::new());
        field_dev
            .reduce_dual_carrier(
                &mut debit_only_field,
                &mut grants,
                [&mut e0, &mut rec],
                &mut tr,
            )
            .expect("metal debit-only dual-carrier");
        let c1_after_bits: Vec<u32> = debit_only_field.as_slice()[c1_start..]
            .iter()
            .map(|v| v.to_bits())
            .collect();
        assert_eq!(
            c1_before_bits, c1_after_bits,
            "grants_debit MUST leave Carrier-1 (planes 25..34) bit-for-bit untouched"
        );
    }

    let mut cpu_reduced = accelerated.current().clone();
    let mut gpu_reduced = accelerated.current().clone();
    let mut cpu_grants = Grants::new();
    resolve_uptake(&cpu_reduced, &feeding_demands, &mut cpu_grants).expect("resolve uptake");
    let mut gpu_grants = cpu_grants.clone();

    let mut cpu_e0 = Deposits::new();
    let mut cpu_rec = Deposits::new();
    let mut cpu_trail = Deposits::new();
    for i in 0..40 {
        let org = OrganismId::new(1_000 + i as u64);
        let pos = [rng.unit() * 64.0, rng.unit() * 64.0];
        let mut dir = [0.0_f64; 32];
        for slot in &mut dir {
            *slot = (rng.unit() * 2.0 - 1.0) * 0.14;
        }
        let paid = 0.2 + rng.unit() * 0.6;
        let split =
            split_dual_carrier_emission(paid, 0.85, trail_rules.trail_emission_share(), &dir)
                .expect("split emission");
        cpu_e0
            .push(
                &grid,
                org,
                pos,
                split.paid_0,
                split.offered_0,
                &split.direction_0,
            )
            .expect("push e0");
        cpu_trail
            .push(
                &grid,
                org,
                pos,
                split.paid_1,
                split.offered_1,
                &split.direction_1,
            )
            .expect("push trail");
        if i % 3 == 0 {
            let mut rec_dir = [0.0_f64; 32];
            for slot in rec_dir.iter_mut().take(CARRIER0_MOMENT_CHANNELS as usize) {
                *slot = (rng.unit() * 2.0 - 1.0) * 0.12;
            }
            let rec_paid = 0.3 + rng.unit() * 0.5;
            cpu_rec
                .push(&grid, org, pos, rec_paid, rec_paid * 0.9, &rec_dir)
                .expect("push rec");
        }
    }
    let mut gpu_e0 = cpu_e0.clone();
    let mut gpu_rec = cpu_rec.clone();
    let mut gpu_trail = cpu_trail.clone();

    let (want_c0_sum, want_c1_sum) = reduce_dual_carrier_field_on(
        &mut cpu_reduced,
        &mut cpu_grants,
        [&mut cpu_e0, &mut cpu_rec],
        &mut cpu_trail,
        4,
    )
    .expect("cpu reduce_dual_carrier");
    let (got_c0_sum, got_c1_sum) = field_dev
        .reduce_dual_carrier(
            &mut gpu_reduced,
            &mut gpu_grants,
            [&mut gpu_e0, &mut gpu_rec],
            &mut gpu_trail,
        )
        .expect("metal reduce_dual_carrier");

    let summary_bits = |s: DepositSummary| {
        (
            s.organisms,
            s.cells,
            s.paid.to_bits(),
            s.offered.to_bits(),
            s.applied.to_bits(),
        )
    };
    assert_eq!(summary_bits(want_c0_sum[0]), summary_bits(got_c0_sum[0]));
    assert_eq!(summary_bits(want_c0_sum[1]), summary_bits(got_c0_sum[1]));
    assert_eq!(summary_bits(want_c1_sum), summary_bits(got_c1_sum));
    assert!(got_c1_sum.applied > 0.0, "expected positive trail deposits");
    let reduce_diff = cpu_reduced
        .as_slice()
        .iter()
        .zip(gpu_reduced.as_slice())
        .filter(|(a, b)| a.to_bits() != b.to_bits())
        .count();
    assert_eq!(
        reduce_diff, 0,
        "reduce_dual_carrier: {reduce_diff} values differ across 34 planes"
    );

    // -------------------------------------------------------------------------
    // 5. check.metal: valid 34-plane scan, Carrier-0 fault, Carrier-1-only cone
    //    fault, and Carrier-1-only negative e_1 fault all match CPU bit-for-bit.
    // -------------------------------------------------------------------------
    let tolerance = ConeTolerance::new(1.0e-5, 1.0e-5).expect("tolerance");
    let scan_bits = |s: flife_field::FieldScan| {
        (
            s.offending,
            s.largest.map(|(cell, res)| (cell, res.to_bits())),
        )
    };
    let want_valid = cpu_reduced.scan_invariants(tolerance);
    let got_valid = field_dev
        .check(&gpu_reduced, tolerance)
        .expect("metal check valid 34-plane");
    assert_eq!(scan_bits(want_valid), scan_bits(got_valid));
    assert_eq!(got_valid.offending, None);
    assert_eq!(
        gpu_reduced
            .summarize_scan(tolerance, &got_valid)
            .expect("summarize gpu scan"),
        Some(cpu_reduced.check_invariants(tolerance).expect("cpu check")),
    );

    // Carrier-1-only cone violation (Carrier 0 completely valid).
    let mut c1_cone_fault = gpu_reduced.clone();
    let bad_cell = 1_337_usize;
    c1_cone_fault.trail_energy_mut().expect("e1")[bad_cell] = 0.25;
    c1_cone_fault.trail_moment_mut(0).expect("c1 m0")[bad_cell] = 0.5;
    let want_c1_cone = c1_cone_fault.scan_invariants(tolerance);
    let got_c1_cone = field_dev
        .check(&c1_cone_fault, tolerance)
        .expect("metal check c1 cone fault");
    assert_eq!(want_c1_cone.offending, Some(bad_cell));
    assert_eq!(scan_bits(got_c1_cone), scan_bits(want_c1_cone));

    // Carrier-1-only negative energy violation.
    let mut c1_neg_fault = gpu_reduced.clone();
    c1_neg_fault.trail_energy_mut().expect("e1")[777] = -0.01;
    let want_c1_neg = c1_neg_fault.scan_invariants(tolerance);
    let got_c1_neg = field_dev
        .check(&c1_neg_fault, tolerance)
        .expect("metal check c1 neg fault");
    assert_eq!(want_c1_neg.offending, Some(777));
    assert_eq!(scan_bits(got_c1_neg), scan_bits(want_c1_neg));

    // -------------------------------------------------------------------------
    // 6. controller.metal: 108-input Bundle B (`108 -> [64, 32] -> 42`) F32 & F16
    //    fed real observe_dual_for_controller inputs -> 0 deferrals, bit-exact.
    // -------------------------------------------------------------------------
    let topo = DenseTopology::with_io(108, &[64, 32], 42).expect("bundle B topo");
    let mut p32_bank = Vec::with_capacity(64);
    let mut p16_bank = Vec::with_capacity(64);
    for _ in 0..64 {
        let mut p32 = vec![0.0_f32; topo.parameter_count()];
        for layer in topo.layers() {
            let scale = 1.0 / (layer.inputs as f64).sqrt();
            for v in &mut p32[layer.weights..layer.biases + layer.outputs] {
                let raw = rng.unit() * 2.0 - 1.0;
                let nudged = if raw.abs() < 1.0e-3 { 0.25 } else { raw };
                *v = (nudged * scale) as f32;
            }
        }
        let p16: Vec<F16> = p32.iter().map(|&v| F16::from_f64(f64::from(v))).collect();
        p32_bank.push(p32);
        p16_bank.push(p16);
    }
    let mut jobs_f32 = Vec::with_capacity(64);
    let mut jobs_f16 = Vec::with_capacity(64);
    for i in 0..64 {
        jobs_f32.push(ControllerJob {
            genome: GenomeId::new(10_000 + i as u64),
            slot: ArenaSlot::new(0, i as u64, 1),
            controller: ReferenceController::new(&topo, &p32_bank[i], 10.0).expect("ref f32"),
            inputs: &sensor_108[i],
        });
        jobs_f16.push(ControllerJob {
            genome: GenomeId::new(20_000 + i as u64),
            slot: ArenaSlot::new(0, i as u64, 1),
            controller: ReferenceController::new(&topo, &p16_bank[i], 10.0).expect("ref f16"),
            inputs: &sensor_108[i],
        });
    }
    let mut out_f32 = Vec::new();
    let mut out_f16 = Vec::new();
    ctrl_f32
        .evaluate(&jobs_f32, &mut out_f32)
        .expect("metal f32");
    ctrl_f16
        .evaluate(&jobs_f16, &mut out_f16)
        .expect("metal f16");
    let mut scratch = ControllerScratch::new(&topo).expect("scratch");
    for i in 0..64 {
        let got32 = out_f32[i].expect("0 F32 deferrals on Bundle B 108->42");
        let want32 = jobs_f32[i]
            .controller
            .evaluate_raw(&sensor_108[i], &mut scratch)
            .expect("eval f32");
        assert_eq!(
            got32.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            want32.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            "Bundle B F32 controller org={i} differs"
        );
        let got16 = out_f16[i].expect("0 F16 deferrals on Bundle B 108->42");
        let want16 = jobs_f16[i]
            .controller
            .evaluate_raw(&sensor_108[i], &mut scratch)
            .expect("eval f16");
        assert_eq!(
            got16.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            want16.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            "Bundle B F16 controller org={i} differs"
        );
    }

    // -------------------------------------------------------------------------
    // 7. Negative refusals for 33/34-plane mismatches.
    // -------------------------------------------------------------------------
    assert!(matches!(
        field_dev.advance_wave2(
            &mut accelerated,
            &c0_plan,
            &base_kernel,
            &sources,
            step,
            FieldStepPhase2::BASELINE,
        ),
        Err(CoreError::LayoutMismatch { .. })
    ));
    let mut single_field = FieldState::zeroed(grid, &LIMITS).expect("33-plane");
    let mut empty_grants = Grants::new();
    let (mut d0, mut d1, mut dt) = (Deposits::new(), Deposits::new(), Deposits::new());
    assert!(matches!(
        field_dev.reduce_dual_carrier(
            &mut single_field,
            &mut empty_grants,
            [&mut d0, &mut d1],
            &mut dt,
        ),
        Err(CoreError::LayoutMismatch { .. })
    ));
    let mut dummy_dual = Vec::new();
    assert!(matches!(
        sensors_dev.sample_dual(&pyramid, &single_field, &queries[..4], &mut dummy_dual),
        Err(CoreError::LayoutMismatch { .. })
    ));
    let mut dummy_single = Vec::new();
    assert!(matches!(
        sensors_dev.sample(
            &pyramid,
            accelerated.current(),
            &queries[..4],
            &mut dummy_single
        ),
        Err(CoreError::LayoutMismatch { .. })
    ));
}
