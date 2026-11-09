//! The fused forward pass against the reference controller, organism by
//! organism and bit for bit, over every catalog topology in one mixed batch.

use flife_backend_metal::{Batch, MetalControllers};
use flife_core::CoreError;
use flife_neural::{
    ACTION_WIDTH, ControllerScratch, DenseTopology, ReferenceController, SENSOR_WIDTH,
};

/// The `dense-104-36-v1` catalog's hidden widths.
const TOPOLOGIES: [&[u32]; 5] = [&[8], &[32], &[64, 32], &[96, 64], &[128, 96]];

/// A bound admitting every finite F32 parameter.
const BOUND: f64 = f32::MAX as f64;

#[expect(
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn controllers() -> Option<MetalControllers> {
    match MetalControllers::open() {
        Ok(controllers) => Some(controllers),
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => None,
        Err(error) => panic!("Metal controllers did not open: {error}"),
    }
}

/// SplitMix64, and uniform draws from it.
struct Draws(u64);

impl Draws {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[-1, 1)`.
    fn symmetric(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1_u64 << 52) as f64 - 1.0
    }
}

/// How an organism's parameters and inputs are drawn.
#[derive(Debug, Clone, Copy)]
enum Regime {
    /// Fan-in-scaled weights and observation-sized inputs.
    Typical,
    /// Weights eight times larger: many activations saturate.
    Saturating,
    /// Small weights and inputs whose products stay above 2^-102: normal
    /// arithmetic far below one, which the device must not defer.
    Small,
    /// Subnormal inputs, unit weights and zero biases: subnormal products
    /// and sums, which the device would flush and so defers.
    Tiny,
    /// Hidden pre-activations overflow F32.
    HiddenOverflow,
    /// Only the linear outputs overflow.
    OutputOverflow,
}

fn parameters(topology: &DenseTopology, regime: Regime, draws: &mut Draws) -> Vec<f32> {
    let mut values = vec![0.0_f32; topology.parameter_count()];
    let last = topology.layers().len() - 1;
    for (index, layer) in topology.layers().iter().enumerate() {
        let scale = match regime {
            Regime::Typical => 2.0 / (layer.inputs as f64).sqrt(),
            Regime::Saturating => 16.0 / (layer.inputs as f64).sqrt(),
            Regime::Small => 1.0e-9,
            Regime::Tiny => 1.0,
            Regime::HiddenOverflow if index == 0 => 1.0e30,
            Regime::OutputOverflow if index == last => 3.0e38,
            Regime::HiddenOverflow | Regime::OutputOverflow => 1.0,
        };
        let end = layer.biases + layer.outputs;
        for value in &mut values[layer.weights..end] {
            *value = (draws.symmetric() * scale) as f32;
        }
        if matches!(regime, Regime::Tiny) {
            values[layer.biases..end].fill(0.0);
        }
        if matches!(regime, Regime::OutputOverflow) && index == last {
            for value in &mut values[layer.weights..end] {
                *value = value.abs();
            }
        }
    }
    values
}

fn inputs(regime: Regime, draws: &mut Draws) -> [f64; SENSOR_WIDTH] {
    let mut inputs = [0.0_f64; SENSOR_WIDTH];
    for (index, input) in inputs.iter_mut().enumerate() {
        *input = match (regime, index % 7) {
            // Exact zeros, as an empty channel gives.
            (_, 0) if draws.next().is_multiple_of(3) => 0.0,
            (Regime::Small, _) => draws.symmetric() * 1.0e-9,
            (Regime::Tiny, _) => draws.symmetric() * 1.0e-39,
            (Regime::HiddenOverflow, _) => draws.symmetric().abs() * 1.0e10 + 1.0,
            // Unnarrowed F64 values: rounding to F32 is part of the contract.
            (_, 1) => draws.symmetric() * 40.0,
            _ => draws.symmetric() * 2.0,
        };
    }
    inputs
}

#[test]
fn every_topology_matches_the_reference_bit_for_bit_in_one_mixed_batch() -> Result<(), CoreError> {
    let Some(controllers) = controllers() else {
        return Ok(());
    };
    let topologies: Vec<DenseTopology> = TOPOLOGIES
        .iter()
        .map(|widths| DenseTopology::new(widths))
        .collect::<Result<_, _>>()?;
    let mut draws = Draws(2026);
    let mut cases = Vec::new();
    let mut batch = Batch::new();
    for index in 0..2_000 {
        let topology = &topologies[(draws.next() % 5) as usize];
        let regime = match index % 20 {
            0..=10 => Regime::Typical,
            11..=14 => Regime::Saturating,
            15 => Regime::Small,
            16 | 17 => Regime::Tiny,
            18 => Regime::HiddenOverflow,
            _ => Regime::OutputOverflow,
        };
        let parameters = parameters(topology, regime, &mut draws);
        let inputs = inputs(regime, &mut draws);
        batch.push(topology, &parameters, &inputs)?;
        cases.push((topology, regime, parameters, inputs));
    }
    assert_eq!(batch.len(), cases.len());

    let mut outputs = Vec::new();
    controllers.evaluate(&batch, &mut outputs)?;
    assert_eq!(outputs.len(), cases.len());

    let mut refused = [0_usize; 2];
    let mut deferred = 0_usize;
    for (index, ((topology, regime, parameters, inputs), got)) in
        cases.iter().zip(&outputs).enumerate()
    {
        let mut scratch = ControllerScratch::new(topology)?;
        let reference = ReferenceController::new(topology, parameters, BOUND)?;
        match (reference.evaluate(inputs, &mut scratch), got) {
            (Ok(want), Some(got)) => {
                let differs = (0..ACTION_WIDTH).find(|&k| want[k].to_bits() != got[k].to_bits());
                assert!(
                    differs.is_none(),
                    "organism {index} ({:?}, {regime:?}): output {differs:?} is {:?}, not {:?}",
                    topology.hidden_widths(),
                    differs.map(|k| got[k]),
                    differs.map(|k| want[k]),
                );
            }
            (Err(CoreError::NumericalInvalid { quantity, .. }), None) => {
                refused[usize::from(quantity == "controller output")] += 1;
            }
            // Deferred for the subnormal range, which only tiny arithmetic
            // reaches; the reference gives the value.
            (Ok(_), None) if matches!(regime, Regime::Tiny) => deferred += 1,
            (want, got) => {
                panic!("organism {index} ({regime:?}): reference {want:?}, Metal {got:?}")
            }
        }
    }
    // Both refusals and the deferral were exercised, so agreement on them is
    // not vacuous.
    assert!(refused[0] > 0 && refused[1] > 0, "refusals {refused:?}");
    assert!(deferred > 0, "no organism was deferred");
    Ok(())
}

#[test]
fn a_batch_refuses_what_the_reference_refuses_and_stays_unchanged() -> Result<(), CoreError> {
    let topology = DenseTopology::new(&[64, 32])?;
    let mut draws = Draws(1);
    let parameters = parameters(&topology, Regime::Typical, &mut draws);
    let mut inputs = inputs(Regime::Typical, &mut draws);
    let mut batch = Batch::new();
    batch.push(&topology, &parameters, &inputs)?;

    // An input finite in F64 and infinite in F32.
    inputs[17] = 1.0e39;
    let mut scratch = ControllerScratch::new(&topology)?;
    let want =
        ReferenceController::new(&topology, &parameters, BOUND)?.evaluate(&inputs, &mut scratch);
    let got = batch.push(&topology, &parameters, &inputs);
    assert!(
        matches!(want, Err(CoreError::NumericalInvalid { .. })),
        "{want:?}"
    );
    assert_eq!(got.err(), want.err());

    // Parameters for another topology.
    let other = DenseTopology::new(&[32])?;
    let refused = batch.push(&other, &parameters, &[0.0; SENSOR_WIDTH]);
    assert!(
        matches!(refused, Err(CoreError::LayoutMismatch { .. })),
        "{refused:?}"
    );
    assert_eq!(batch.len(), 1);
    Ok(())
}
