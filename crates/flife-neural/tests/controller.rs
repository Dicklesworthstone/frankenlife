//! FL-008 evidence (plan §23.1): hand-set networks and independent known
//! answers for the scalar reference evaluator (§6.2, §6.5, §10.1, §10.3,
//! Appendix C.11, C.16).
//!
//! The expected values do not come from the evaluator. Hand-set networks
//! use dyadic weights, so every pre-activation is exact, and their `tanh`
//! values were computed in 60-digit decimal arithmetic and rounded directly
//! to the nearest F32 (each is at least 0.10 F32 ulp from a rounding
//! midpoint, so any `tanh` within a few F64 ulp rounds to the same bits).
//! Random networks are checked against a separately written input-major
//! F64 oracle under a rigorous running-error bound.

use flife_core::model::dense_parameter_count;
use flife_core::{CoreError, TimeStep, ValidatedConfig};
use flife_field::{BufferLimits, FieldGrid, FieldState, SensorPyramid};
use flife_neural::actions::{CHILD_SHARE, EMISSION_DIRECTION, EMISSION_GATE, EMISSION_SPEND};
use flife_neural::{
    ACTION_WIDTH, ActionDecoding, ControllerScratch, DenseTopology, Intrinsic, LayerShape,
    ReferenceController, SENSOR_WIDTH, SensorScales, decode, observe, widen,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// Appendix A's `maximum_absolute_parameter`.
const BOUND: f64 = 4.0;

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn appendix_a() -> ValidatedConfig {
    let appendix = &PLAN[PLAN.find("## Appendix A").expect("Appendix A heading")..];
    let start = appendix.find("```toml\n").expect("Appendix A TOML block") + "```toml\n".len();
    let length = appendix[start..].find("```").expect("closing fence");
    ValidatedConfig::from_toml(&appendix[start..start + length]).expect("Appendix A is valid")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn topology(hidden: &[u32]) -> DenseTopology {
    DenseTopology::new(hidden).expect("an admitted topology")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn layer(topology: &DenseTopology, index: usize) -> LayerShape {
    *topology.layers().get(index).expect("layer")
}

/// Set `weight[output][input]` of layer `index`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn set_weight(
    parameters: &mut [f32],
    topology: &DenseTopology,
    index: usize,
    output: usize,
    input: usize,
    value: f32,
) {
    let offset = layer(topology, index)
        .weight(output, input)
        .expect("weight in range");
    parameters[offset] = value;
}

/// Set `bias[output]` of layer `index`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn set_bias(
    parameters: &mut [f32],
    topology: &DenseTopology,
    index: usize,
    output: usize,
    value: f32,
) {
    let offset = layer(topology, index).bias(output).expect("bias in range");
    parameters[offset] = value;
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn evaluate(
    topology: &DenseTopology,
    parameters: &[f32],
    inputs: &[f64; SENSOR_WIDTH],
) -> [f32; ACTION_WIDTH] {
    let controller = ReferenceController::new(topology, parameters, BOUND).expect("controller");
    let mut scratch = ControllerScratch::new(topology).expect("scratch");
    controller.evaluate(inputs, &mut scratch).expect("evaluate")
}

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Uniform on `[0, 1)` from the leading 53 bits.
fn uniform(state: &mut u64) -> f64 {
    (splitmix(state) >> 11) as f64 / 9_007_199_254_740_992.0
}

/// A.3 item 2's fan-in-scaled weights, uniform on `[−√(3/I), √(3/I))`
/// rounded once to F32, with biases uniform on `[−½, ½)` so that they
/// matter.
fn random_parameters(topology: &DenseTopology, seed: u64) -> Vec<f32> {
    let mut state = seed;
    let mut parameters = vec![0.0_f32; topology.parameter_count()];
    for shape in topology.layers() {
        let limit = (3.0 / shape.inputs as f64).sqrt();
        for slot in &mut parameters[shape.weights..shape.biases] {
            *slot = (limit * (2.0 * uniform(&mut state) - 1.0)) as f32;
        }
        for slot in &mut parameters[shape.biases..shape.end()] {
            *slot = (uniform(&mut state) - 0.5) as f32;
        }
    }
    parameters
}

fn random_inputs(state: &mut u64) -> [f64; SENSOR_WIDTH] {
    let mut inputs = [0.0; SENSOR_WIDTH];
    for slot in &mut inputs {
        *slot = 2.0 * uniform(state) - 1.0;
    }
    inputs
}

#[test]
fn c11_zero_weights_pass_output_biases_through_a_linear_layer() {
    // A real observation from a field, so that the inputs are whatever the
    // ABI produces; with every weight zero they must not matter.
    let config = appendix_a();
    let grid = FieldGrid::new(16, 16, 16.0, 16.0).expect("grid");
    let mut field = FieldState::zeroed(grid, &BufferLimits::host()).expect("field");
    for (cell, energy) in field.energy_mut().iter_mut().enumerate() {
        *energy = 1.0 + (cell % 7) as f32;
    }
    let mut storage = SensorPyramid::new(grid, 2, &BufferLimits::host()).expect("pyramid");
    let view = storage.build(&field).expect("build");
    let scales = SensorScales::from_config(&config).expect("scales");
    let intrinsic = Intrinsic {
        energy: 3.0,
        age_seconds: 12.0,
        velocity: [0.5, -0.25],
        energy_change_per_second: -0.1,
    };
    let observation = observe(
        &view.sample([5.3, 9.1], 1.5).expect("sample"),
        &intrinsic,
        &scales,
    )
    .expect("observe");
    assert!(observation.inputs.iter().any(|&input| input != 0.0));

    // Hidden biases are nonzero, so hidden activations are too; zero
    // output weights must still leave exactly the output biases.
    let default = DenseTopology::from_config(&config).expect("Appendix A topology");
    let zero_weights = |logits: &[f32; ACTION_WIDTH]| {
        let mut parameters = vec![0.0_f32; default.parameter_count()];
        for output in 0..64 {
            set_bias(&mut parameters, &default, 0, output, 3.0);
        }
        for output in 0..32 {
            set_bias(&mut parameters, &default, 1, output, -2.0);
        }
        for (output, &logit) in logits.iter().enumerate() {
            set_bias(&mut parameters, &default, 2, output, logit);
        }
        parameters
    };
    let decoding = ActionDecoding::from_config(&config).expect("decoding");
    let dt = TimeStep::new(1, 128).expect("Δt");

    // Appendix A admits |p| ≤ 4, so an output bias alone reaches logits of
    // at most ±4. σ(4) = 0.98201379; a tanh output layer would instead give
    // tanh(4) ≈ 0.99932930 and then σ(tanh 4) ≈ 0.7310.
    let mut logits = [0.0_f32; ACTION_WIDTH];
    logits[0] = -4.0;
    logits[CHILD_SHARE] = -4.0;
    logits[EMISSION_GATE] = 4.0;
    logits[EMISSION_DIRECTION] = 3.0;
    logits[EMISSION_DIRECTION + 1] = 4.0;
    logits[EMISSION_SPEND] = 0.0;
    let raw = evaluate(&default, &zero_weights(&logits), &observation.inputs);
    assert_eq!(raw.map(f32::to_bits), logits.map(f32::to_bits));
    let decoded = decode(&widen(&raw), dt, &decoding).expect("decode");
    assert!((decoded.emission_gate - 0.982_013_790_037_908_5).abs() < 1.0e-15);
    assert!((decoded.child_fraction - (0.1 + 0.8 * 0.017_986_209_962_091_56)).abs() < 1.0e-15);
    let emission = decoded
        .emission
        .expect("an open gate and a valid direction emit");
    assert!((emission.direction[0] - 0.6).abs() <= f64::EPSILON);
    assert!((emission.direction[1] - 0.8).abs() <= f64::EPSILON);

    // C.11's literal logit of 8 is outside Appendix A's bound, which is a
    // run's admission rule (A.3 item 2), not part of the evaluator's
    // arithmetic. As an evaluator-and-decoder fixture it runs under a
    // declared fixture bound of 8.
    logits[CHILD_SHARE] = -8.0;
    logits[EMISSION_GATE] = 8.0;
    let parameters = zero_weights(&logits);
    assert!(ReferenceController::new(&default, &parameters, BOUND).is_err());
    let controller = ReferenceController::new(&default, &parameters, 8.0).expect("fixture bound");
    let mut scratch = ControllerScratch::new(&default).expect("scratch");
    let raw = controller
        .evaluate(&observation.inputs, &mut scratch)
        .expect("evaluate");
    assert_eq!(raw.map(f32::to_bits), logits.map(f32::to_bits));
    // A tanh output layer would have given tanh(8) ≈ 0.99999977 here.
    assert_eq!(raw[EMISSION_GATE], 8.0);
    let decoded = decode(&widen(&raw), dt, &decoding).expect("decode");
    assert!((decoded.emission_gate - 0.999_664_65).abs() < 5.0e-9);
    assert!((decoded.child_fraction - (0.1 + 0.8 * 0.000_335_350_130_466_478)).abs() < 1.0e-15);

    // Within the bound a logit of 8 is still reachable: a hidden unit
    // saturated at exactly 1 (z = 20) through weight 4, plus bias 4.
    let single = topology(&[1]);
    let mut parameters = vec![0.0_f32; single.parameter_count()];
    for input in 0..5 {
        set_weight(&mut parameters, &single, 0, 0, input, 4.0);
    }
    set_weight(&mut parameters, &single, 1, EMISSION_GATE, 0, 4.0);
    set_bias(&mut parameters, &single, 1, EMISSION_GATE, 4.0);
    let mut ones = [0.0; SENSOR_WIDTH];
    ones[..5].fill(1.0);
    assert_eq!(evaluate(&single, &parameters, &ones)[EMISSION_GATE], 8.0);
}

#[test]
fn hand_set_network_matches_independent_known_answers() {
    // 104 → 2 → 36. Neuron 0: z = 0.25 + 1·x_0 + 2·x_1 = 0.5. Neuron 1:
    // z = −0.5 + 0.5·x_2 − 1·x_103 = −0.375. Output k: bias k/32 − ½,
    // weights (k + 1)/16 and ((k mod 5) − 2)/2.
    let small = topology(&[2]);
    assert_eq!(small.parameter_count(), 318);
    let mut parameters = vec![0.0_f32; 318];
    set_bias(&mut parameters, &small, 0, 0, 0.25);
    set_weight(&mut parameters, &small, 0, 0, 0, 1.0);
    set_weight(&mut parameters, &small, 0, 0, 1, 2.0);
    set_bias(&mut parameters, &small, 0, 1, -0.5);
    set_weight(&mut parameters, &small, 0, 1, 2, 0.5);
    set_weight(&mut parameters, &small, 0, 1, 103, -1.0);
    for k in 0..ACTION_WIDTH {
        set_bias(&mut parameters, &small, 1, k, k as f32 / 32.0 - 0.5);
        set_weight(&mut parameters, &small, 1, k, 0, (k + 1) as f32 / 16.0);
        set_weight(
            &mut parameters,
            &small,
            1,
            k,
            1,
            ((k % 5) as f32 - 2.0) * 0.5,
        );
    }
    // Inputs with zero weights are nonzero: they must contribute nothing.
    let mut inputs = [0.3; SENSOR_WIDTH];
    inputs[0] = 0.5;
    inputs[1] = -0.125;
    inputs[2] = 0.75;
    inputs[103] = 0.25;

    // tanh(0.5) = 0x3eec9a9f and tanh(−0.375) = 0xbeb77a9f in F32; each
    // output is ((b + w_0 h_0) + w_1 h_1) in F32.
    const EXPECTED: [u32; ACTION_WIDTH] = [
        0xbde6_eedc,
        0xbe6d_5eb9,
        0xbeb3_a302,
        0xbef0_96a8,
        0xbf16_c527,
        0x3e40_6935,
        0x3d8d_03d4,
        0xbd4d_9584,
        0xbe2d_4cad,
        0xbe93_99fc,
        0x3efa_24ec,
        0x3ebd_3146,
        0x3e80_3da1,
        0x3e06_93f7,
        0x3c4a_cac0,
        0x3f4a_0a9f,
        0x3f2b_90cc,
        0x3f0d_16f9,
        0x3edd_3a4c,
        0x3ea0_46a7,
        0x3f8b_8164,
        0x3f78_88f5,
        0x3f5a_0f22,
        0x3f3b_954f,
        0x3f1d_1b7c,
        0x3fb1_fd78,
        0x3fa2_c08f,
        0x3f93_83a6,
        0x3f84_46bc,
        0x3f6a_13a4,
        0x3fd8_798d,
        0x3fc9_3ca4,
        0x3fb9_ffba,
        0x3faa_c2d0,
        0x3f9b_85e7,
        0x3ffe_f5a1,
    ];
    let raw = evaluate(&small, &parameters, &inputs);
    assert_eq!(raw.map(f32::to_bits), EXPECTED);
    assert!((raw[0] - -0.112_760_28).abs() < 1.0e-7);
    assert!((raw[5] - 0.187_901_33).abs() < 1.0e-7);
}

#[test]
fn c16_a_tanh_layer_is_not_an_identity() {
    // 104 → 1 → 36 reading input 0 = 1 gives tanh(1); inserting an identity
    // bridge 1 → 1 with zero bias gives tanh(tanh(1)) (C.16).
    let mut inputs = [0.0; SENSOR_WIDTH];
    inputs[0] = 1.0;

    let shallow = topology(&[1]);
    let mut parameters = vec![0.0_f32; shallow.parameter_count()];
    set_weight(&mut parameters, &shallow, 0, 0, 0, 1.0);
    set_weight(&mut parameters, &shallow, 1, 0, 0, 1.0);
    let once = evaluate(&shallow, &parameters, &inputs);
    assert_eq!(once[0].to_bits(), 0x3f42_f7d6);
    assert!((f64::from(once[0]) - 0.761_594_16).abs() < 1.0e-7);
    assert!(once[1..].iter().all(|&value| value == 0.0));

    let deep = topology(&[1, 1]);
    let mut parameters = vec![0.0_f32; deep.parameter_count()];
    set_weight(&mut parameters, &deep, 0, 0, 0, 1.0);
    set_weight(&mut parameters, &deep, 1, 0, 0, 1.0);
    set_weight(&mut parameters, &deep, 2, 0, 0, 1.0);
    let twice = evaluate(&deep, &parameters, &inputs);
    assert_eq!(twice[0].to_bits(), 0x3f24_5b18);
    assert!((f64::from(twice[0]) - 0.642_014_99).abs() < 1.0e-7);
    assert!(twice[0] < once[0] - 0.1);
}

#[test]
fn accumulation_is_f32_bias_first_in_ascending_input_order() {
    // b = 2^-22 with products 4·1 and −4·1. In F32, 4 + 2^-22 is a tie that
    // rounds to 4, and 2^-22 − 4 is exact. The declared order
    // ((b + p_0) + p_1) therefore gives 0 for (4, −4) and 2^-22 for
    // (−4, 4). Products first would give 2^-22 for both, descending input
    // order 0 for both, and F64 accumulation 2^-22 for both.
    let tie = 2.0_f32.powi(-22);
    let mut inputs = [0.0; SENSOR_WIDTH];
    inputs[0] = 1.0;
    inputs[1] = 1.0;
    let single = topology(&[1]);
    for (first, second, expected) in [(4.0, -4.0, 0.0), (-4.0, 4.0, tie)] {
        let mut parameters = vec![0.0_f32; single.parameter_count()];
        set_bias(&mut parameters, &single, 0, 0, tie);
        set_weight(&mut parameters, &single, 0, 0, 0, first);
        set_weight(&mut parameters, &single, 0, 0, 1, second);
        set_weight(&mut parameters, &single, 1, 0, 0, 1.0);
        // tanh(2^-22) rounds to 2^-22 in F32, and tanh(0) is 0.
        let raw = evaluate(&single, &parameters, &inputs);
        assert_eq!(raw[0].to_bits(), expected.to_bits(), "({first}, {second})");
    }

    // Inputs are rounded once to F32: 1 + 2^-30 is 1 there.
    let mut parameters = vec![0.0_f32; single.parameter_count()];
    set_weight(&mut parameters, &single, 0, 0, 0, 1.0);
    set_weight(&mut parameters, &single, 0, 0, 1, -1.0);
    set_weight(&mut parameters, &single, 1, 0, 0, 1.0);
    inputs[0] = 1.0 + 2.0_f64.powi(-30);
    assert_eq!(evaluate(&single, &parameters, &inputs)[0], 0.0);
}

#[test]
fn parameters_follow_the_declared_layout() {
    // 104 → 3 → 2 → 36: weight[out][in] row-major, then bias[out], per
    // layer in evaluation order.
    let net = topology(&[3, 2]);
    assert_eq!(net.parameter_count(), 104 * 3 + 3 + 3 * 2 + 2 + 2 * 36 + 36);
    assert_eq!(net.parameter_count(), 431);
    let expected = [
        LayerShape {
            inputs: 104,
            outputs: 3,
            weights: 0,
            biases: 312,
        },
        LayerShape {
            inputs: 3,
            outputs: 2,
            weights: 315,
            biases: 321,
        },
        LayerShape {
            inputs: 2,
            outputs: 36,
            weights: 323,
            biases: 395,
        },
    ];
    assert_eq!(net.layers(), &expected);
    assert_eq!(expected[2].end(), 431);
    assert_eq!(expected[0].weight(2, 103), Some(311));
    assert_eq!(expected[0].bias(2), Some(314));
    assert_eq!(expected[1].weight(1, 2), Some(320));
    assert_eq!(expected[1].bias(1), Some(322));
    assert_eq!(expected[2].weight(35, 1), Some(394));
    assert_eq!(expected[2].bias(35), Some(430));
    assert_eq!(expected[0].weight(3, 0), None);
    assert_eq!(expected[0].weight(0, 104), None);
    assert_eq!(expected[2].bias(36), None);

    // Hidden biases only: h_0 = tanh(0.5, −0.25, 1) and
    // h_1 = tanh(0.75, −0.5); every output is 0.
    let mut parameters = vec![0.0_f32; 431];
    for (output, bias) in [0.5, -0.25, 1.0].into_iter().enumerate() {
        set_bias(&mut parameters, &net, 0, output, bias);
    }
    set_bias(&mut parameters, &net, 1, 0, 0.75);
    set_bias(&mut parameters, &net, 1, 1, -0.5);
    let inputs = [0.5; SENSOR_WIDTH];
    assert!(
        evaluate(&net, &parameters, &inputs)
            .iter()
            .all(|&value| value == 0.0)
    );

    // Each edit reaches exactly the outputs its index names.
    set_weight(&mut parameters, &net, 2, 5, 1, 1.0);
    set_bias(&mut parameters, &net, 2, 7, 2.0);
    set_weight(&mut parameters, &net, 2, 9, 0, 1.0);
    let raw = evaluate(&net, &parameters, &inputs);
    assert_eq!(raw[5].to_bits(), 0xbeec_9a9f, "tanh(−0.5)");
    assert_eq!(raw[7], 2.0);
    assert_eq!(raw[9].to_bits(), 0x3f22_991f, "tanh(0.75)");
    for (output, &value) in raw.iter().enumerate() {
        if ![5, 7, 9].contains(&output) {
            assert_eq!(value, 0.0, "output {output}");
        }
    }
    // A middle-layer weight from h_0[2] = tanh(1) into h_1[0] moves only
    // output 9: tanh(0.75 + tanh(1)) = 0x3f683fac.
    set_weight(&mut parameters, &net, 1, 0, 2, 1.0);
    let moved = evaluate(&net, &parameters, &inputs);
    assert_eq!(moved[9].to_bits(), 0x3f68_3fac);
    assert_eq!(moved[5], raw[5]);
    assert_eq!(moved[7], raw[7]);
}

/// An input-major F64 evaluation with `std`'s `tanh`, and for every output a
/// bound on how far an F32 evaluation in any order can lie from it: the
/// classical `γ_n Σ|w x|` for each dot product, the propagated input
/// error, F32 rounding of inputs and activations, and a few F64 ulp for
/// the activation itself.
fn oracle(
    topology: &DenseTopology,
    parameters: &[f32],
    inputs: &[f64; SENSOR_WIDTH],
) -> (Vec<f64>, Vec<f64>) {
    let unit = f64::from(f32::EPSILON) / 2.0;
    let mut values = inputs.to_vec();
    let mut errors: Vec<f64> = inputs.iter().map(|value| unit * value.abs()).collect();
    let layers = topology.layers();
    for (index, shape) in layers.iter().enumerate() {
        let mut z: Vec<f64> = parameters[shape.biases..shape.end()]
            .iter()
            .map(|&bias| f64::from(bias))
            .collect();
        let mut magnitude: Vec<f64> = z.iter().map(|bias| bias.abs()).collect();
        let mut propagated = vec![0.0; shape.outputs];
        for input in 0..shape.inputs {
            for output in 0..shape.outputs {
                let weight = f64::from(parameters[shape.weights + output * shape.inputs + input]);
                z[output] += weight * values[input];
                magnitude[output] += (weight * values[input]).abs();
                propagated[output] += weight.abs() * errors[input];
            }
        }
        let terms = (shape.inputs + 1) as f64;
        let gamma = terms * unit / (1.0 - terms * unit);
        let bound: Vec<f64> = magnitude
            .iter()
            .zip(&propagated)
            .map(|(&magnitude, &propagated)| gamma * (magnitude + propagated) + propagated)
            .collect();
        if index + 1 == layers.len() {
            return (z, bound);
        }
        values = z.iter().map(|z| z.tanh()).collect();
        errors = bound
            .iter()
            .zip(&values)
            .map(|(&error, &h)| error + unit * (h.abs() + error) + 4.0 * f64::EPSILON)
            .collect();
    }
    (Vec::new(), Vec::new())
}

#[test]
fn agrees_with_an_independent_f64_oracle_within_a_running_error_bound() {
    let mut state = 0x5eed_u64;
    let mut largest_error = 0.0_f64;
    let mut largest_ratio = 0.0_f64;
    for hidden in [&[64, 32][..], &[8], &[128, 96], &[1, 7]] {
        let net = topology(hidden);
        for seed in 0..6 {
            let parameters = random_parameters(&net, seed * 7919 + hidden[0] as u64);
            for _ in 0..4 {
                let inputs = random_inputs(&mut state);
                let raw = evaluate(&net, &parameters, &inputs);
                let (expected, bound) = oracle(&net, &parameters, &inputs);
                for output in 0..ACTION_WIDTH {
                    let error = (f64::from(raw[output]) - expected[output]).abs();
                    assert!(
                        error <= bound[output] + 1.0e-12,
                        "{hidden:?} output {output}: error {error:e} above bound {:e}",
                        bound[output]
                    );
                    largest_error = largest_error.max(error);
                    largest_ratio = largest_ratio.max(error / bound[output]);
                }
            }
        }
    }
    eprintln!("largest error {largest_error:e}; largest error/bound {largest_ratio:.3}");
    // The evaluation really is F32 (it differs from F64), and the bound is
    // not vacuous.
    assert!(largest_error > 1.0e-9, "{largest_error:e}");
    assert!(largest_error < 1.0e-5, "{largest_error:e}");
    assert!(largest_ratio < 0.5, "{largest_ratio}");
}

#[test]
fn controllers_keep_their_own_weights_and_scratch_carries_nothing() {
    // INV-13: one topology, independent weights. The same observation
    // through two genomes gives two answers; a shared scratch never leaks
    // one evaluation into the next, whatever its previous width.
    let default = topology(&[64, 32]);
    let narrow = topology(&[8]);
    let wide = topology(&[128, 96]);
    let first = random_parameters(&default, 1);
    let second = random_parameters(&default, 2);
    let narrow_parameters = random_parameters(&narrow, 3);
    let wide_parameters = random_parameters(&wide, 4);
    let mut state = 11_u64;
    let inputs = random_inputs(&mut state);

    let a = ReferenceController::new(&default, &first, BOUND).expect("a");
    let b = ReferenceController::new(&default, &second, BOUND).expect("b");
    let n = ReferenceController::new(&narrow, &narrow_parameters, BOUND).expect("narrow");
    let w = ReferenceController::new(&wide, &wide_parameters, BOUND).expect("wide");
    let mut shared = ControllerScratch::default();
    let a_first = a.evaluate(&inputs, &mut shared).expect("a");
    let b_value = b.evaluate(&inputs, &mut shared).expect("b");
    let w_value = w.evaluate(&inputs, &mut shared).expect("wide");
    let n_value = n.evaluate(&inputs, &mut shared).expect("narrow");
    let a_again = a.evaluate(&inputs, &mut shared).expect("a again");
    assert_eq!(a_first.map(f32::to_bits), a_again.map(f32::to_bits));
    assert_ne!(a_first, b_value);
    for (controller, value, net) in [
        (&b, b_value, &default),
        (&w, w_value, &wide),
        (&n, n_value, &narrow),
    ] {
        let mut fresh = ControllerScratch::new(net).expect("scratch");
        let alone = controller.evaluate(&inputs, &mut fresh).expect("alone");
        assert_eq!(alone.map(f32::to_bits), value.map(f32::to_bits));
    }

    fn shareable<T: Send + Sync>() {}
    shareable::<ReferenceController<'_>>();
    shareable::<DenseTopology>();
}

/// FL-021 (plan §14.2's sampled activations): a trace's outputs are
/// `evaluate`'s, bit for bit, and its hidden layers are the declared
/// arithmetic written out here over the layer shapes (the bias, then each
/// input's product in ascending index, in F32; `tanh` in F64 rounded once),
/// one per hidden layer at its width, from a scratch a wider net used last.
#[test]
fn a_trace_keeps_each_hidden_layer_and_the_outputs() {
    let mut shared = ControllerScratch::default();
    let widest = topology(&[128, 96]);
    let widest_parameters = random_parameters(&widest, 9);
    let mut state = 21_u64;
    for (hidden, seed) in [(&[8][..], 5), (&[64, 32], 6), (&[128, 96], 7)] {
        let net = topology(hidden);
        let parameters = random_parameters(&net, seed);
        let inputs = random_inputs(&mut state);
        let controller = ReferenceController::new(&net, &parameters, BOUND).expect("controller");
        ReferenceController::new(&widest, &widest_parameters, BOUND)
            .expect("the widest")
            .evaluate(&inputs, &mut shared)
            .expect("evaluates");
        let trace = controller.trace(&inputs, &mut shared).expect("traces");
        assert_eq!(
            trace.outputs.map(f32::to_bits),
            evaluate(&net, &parameters, &inputs).map(f32::to_bits),
            "{hidden:?}"
        );
        assert_eq!(trace.hidden.len(), hidden.len(), "{hidden:?}");
        let mut source: Vec<f32> = inputs.iter().map(|&input| input as f32).collect();
        for (index, (reported, &width)) in trace.hidden.iter().zip(hidden).enumerate() {
            let shape = layer(&net, index);
            let expected: Vec<f32> = (0..shape.outputs)
                .map(|output| {
                    let mut z = parameters[shape.bias(output).expect("a bias")];
                    for (input, value) in source.iter().enumerate() {
                        z += parameters[shape.weight(output, input).expect("a weight")] * value;
                    }
                    flife_core::numerics::tanh(f64::from(z)).expect("finite") as f32
                })
                .collect();
            assert_eq!(reported.len(), width as usize, "{hidden:?} layer {index}");
            assert_eq!(
                reported
                    .iter()
                    .map(|value| value.to_bits())
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|value| value.to_bits())
                    .collect::<Vec<_>>(),
                "{hidden:?} layer {index}"
            );
            source = expected;
        }
    }
}

#[test]
fn catalog_parameter_counts_match_the_plan() {
    // §6.4's table, biases included.
    for (hidden, count) in [
        (&[8][..], 1_164),
        (&[32], 4_548),
        (&[64, 32], 9_988),
        (&[96, 64], 18_628),
        (&[128, 96], 29_316),
    ] {
        let net = topology(hidden);
        assert_eq!(net.parameter_count(), count, "{hidden:?}");
        assert_eq!(
            dense_parameter_count(104, hidden, 36).expect("count"),
            count as u64
        );
        assert_eq!(net.hidden_widths(), hidden);
        let last = net.layers().last().expect("output layer");
        assert_eq!((last.outputs, last.end()), (ACTION_WIDTH, count));
    }
    let config = appendix_a();
    let default = DenseTopology::from_config(&config).expect("Appendix A topology");
    assert_eq!(default.hidden_widths(), &[64, 32]);
    assert_eq!(
        default.parameter_count() as u64,
        config.report().default_parameter_count
    );
    let offsets: Vec<(usize, usize)> = default
        .layers()
        .iter()
        .map(|shape| (shape.weights, shape.biases))
        .collect();
    assert_eq!(offsets, [(0, 6_656), (6_720, 8_768), (8_800, 9_952)]);
}

#[test]
fn refusals() {
    for hidden in [&[][..], &[8, 8, 8], &[0], &[64, 0]] {
        assert!(
            matches!(
                DenseTopology::new(hidden),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "{hidden:?}"
        );
    }

    let default = topology(&[64, 32]);
    let zeros = vec![0.0_f32; 9_988];
    for length in [9_987, 9_989, 0] {
        let wrong = vec![0.0_f32; length];
        assert!(matches!(
            ReferenceController::new(&default, &wrong, BOUND),
            Err(CoreError::LayoutMismatch { .. })
        ));
    }
    for bound in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(ReferenceController::new(&default, &zeros, bound).is_err());
    }
    let above = f32::from_bits(4.0_f32.to_bits() + 1);
    for (value, admitted) in [
        (4.0, true),
        (-4.0, true),
        (above, false),
        (-above, false),
        (f32::NAN, false),
        (f32::INFINITY, false),
    ] {
        let mut parameters = zeros.clone();
        parameters[1_234] = value;
        assert_eq!(
            ReferenceController::new(&default, &parameters, BOUND).is_ok(),
            admitted,
            "{value}"
        );
    }

    let controller = ReferenceController::new(&default, &zeros, BOUND).expect("controller");
    let mut scratch = ControllerScratch::new(&default).expect("scratch");
    for bad in [f64::NAN, f64::INFINITY, 1.0e39, -f64::MAX] {
        let mut inputs = [0.0; SENSOR_WIDTH];
        inputs[17] = bad;
        assert!(
            matches!(
                controller.evaluate(&inputs, &mut scratch),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "{bad}"
        );
    }
    // A finite F32 input that overflows its pre-activation is refused, not
    // saturated.
    let single = topology(&[1]);
    let mut parameters = vec![0.0_f32; single.parameter_count()];
    set_weight(&mut parameters, &single, 0, 0, 0, 4.0);
    let mut inputs = [0.0; SENSOR_WIDTH];
    inputs[0] = 3.0e38;
    let overflowing = ReferenceController::new(&single, &parameters, BOUND).expect("controller");
    assert!(matches!(
        overflowing.evaluate(&inputs, &mut scratch),
        Err(CoreError::NumericalInvalid { .. })
    ));
    // The largest admitted inputs stay finite.
    inputs[0] = 1.0;
    assert!(overflowing.evaluate(&inputs, &mut scratch).is_ok());
}

#[test]
fn default_network_outputs_are_identical_on_every_host() {
    // A regression pin, not an independent answer: the declared arithmetic
    // uses only IEEE-754 operations and host-independent `tanh`, so every
    // conforming host computes these bits. The Linux and macOS runs check
    // the same constant.
    let default = topology(&[64, 32]);
    let parameters = random_parameters(&default, 0x00f1_1fe0);
    let controller = ReferenceController::new(&default, &parameters, BOUND).expect("controller");
    let mut scratch = ControllerScratch::new(&default).expect("scratch");
    let mut state = 0xc0ffee_u64;
    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
    for _ in 0..64 {
        let inputs = random_inputs(&mut state);
        for value in controller
            .evaluate(&inputs, &mut scratch)
            .expect("evaluate")
        {
            for byte in value.to_bits().to_le_bytes() {
                digest = (digest ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    assert_eq!(digest, 0x821a_1b48_e19f_12ec);
}
