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
