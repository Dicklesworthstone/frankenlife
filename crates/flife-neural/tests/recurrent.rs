//! FL-054: independent known-answer, bounded-execution, and schema tests for
//! `independent-recurrent-mlp-v1` (plan §6.2, §17.5, §25.2).

use flife_core::CoreError;
use flife_core::binary16::F16;
use flife_core::numerics::{sigmoid, tanh};
use flife_neural::{
    ACTION_WIDTH, ControllerScratch, DenseTopology, MAXIMUM_RECURRENT_STATE_WIDTH,
    ReferenceController, SENSOR_WIDTH, Weight,
};

/// Independent scalar oracle written straight from §6.2 and §25.2 without
/// sharing production evaluation helpers.
fn independent_recurrent_oracle<W: Weight>(
    topology: &DenseTopology,
    parameters: &[W],
    inputs: &[f64; SENSOR_WIDTH],
    state: &[f32],
) -> ([f32; ACTION_WIDTH], Vec<f32>) {
    let s = topology.recurrent_state_width();
    let mut current: Vec<f32> = inputs.iter().map(|&x| x as f32).collect();
    current.extend_from_slice(state);

    let (output_layer, hidden_layers) = topology.layers().split_last().expect("output layer");
    for layer in hidden_layers {
        let mut next = Vec::with_capacity(layer.outputs);
        for o in 0..layer.outputs {
            let mut z = parameters[layer.bias(o).expect("bias")].widen();
            for i in 0..layer.inputs {
                let w = parameters[layer.weight(o, i).expect("weight")].widen();
                z += w * current[i];
            }
            next.push(tanh(f64::from(z)).expect("tanh") as f32);
        }
        current = next;
    }

    let mut raw_all = Vec::with_capacity(output_layer.outputs);
    for o in 0..output_layer.outputs {
        let mut z = parameters[output_layer.bias(o).expect("bias")].widen();
        for i in 0..output_layer.inputs {
            let w = parameters[output_layer.weight(o, i).expect("weight")].widen();
            z += w * current[i];
        }
        raw_all.push(z);
    }

    let mut actions = [0.0_f32; ACTION_WIDTH];
    actions.copy_from_slice(&raw_all[..ACTION_WIDTH]);

    let mut next_state = Vec::with_capacity(s);
    for k in 0..s {
        let cand = tanh(f64::from(raw_all[ACTION_WIDTH + k])).expect("cand tanh") as f32;
        let gate = sigmoid(f64::from(raw_all[ACTION_WIDTH + s + k])).expect("gate sigmoid") as f32;
        let h_next = ((1.0_f32 - gate) * state[k] + gate * cand).clamp(-1.0_f32, 1.0_f32);
        next_state.push(h_next);
    }
    (actions, next_state)
}

fn pseudo_values(seed: u64, count: usize, scale: f64) -> Vec<f32> {
    let mut x = seed;
    (0..count)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let u = ((x >> 11) as f64) / ((1_u64 << 53) as f64);
            ((2.0 * u - 1.0) * scale) as f32
        })
        .collect()
}

/// Registry evidence for `independent-recurrent-mlp-v1`:
/// 1. Hand-constructed gate-closed, gate-open, and half-gate transitions match
///    exact analytic values.
/// 2. Identical sensor inputs with different internal states produce distinct
///    action outputs.
/// 3. 1-hidden and 2-hidden recurrent topologies in both F32 and F16 match an
///    independent scalar oracle bit-for-bit.
/// 4. Extreme parameters over 200 sequential evaluations remain strictly finite
///    and bounded in `[-1, 1]^S`.
/// 5. Stateless/recurrent misuse and invalid states are explicitly refused.
#[test]
fn gated_recurrent_controller_matches_independent_oracle_and_stays_bounded() {
    // 1. Hand-set analytic gate behavior on state width S = 4, hidden = [8].
    let topo = DenseTopology::new_recurrent(&[8], 4).expect("recurrent topo");
    assert_eq!(topo.input_width(), SENSOR_WIDTH + 4);
    assert_eq!(topo.output_width(), ACTION_WIDTH + 8);
    assert_eq!(
        topo.parameter_count(),
        (108 * 8 + 8) + (8 * 44 + 44),
        "recurrent parameter count must charge state inputs + candidate/gate heads"
    );

    let mut params = vec![0.0_f32; topo.parameter_count()];
    let l0 = topo.layers()[0];
    let l1 = topo.layers()[1];

    // Wire state[0] -> hidden[0] -> action[0] so internal state drives actions.
    params[l0.weight(0, SENSOR_WIDTH).expect("w")] = 1.0;
    params[l1.weight(0, 0).expect("w")] = 2.0;

    // Candidate biases (channels 36..40):
    // k=0: cand = tanh(1.0), gate bias = -30.0 (closed -> retains h[0])
    // k=1: cand = tanh(1.0), gate bias = +30.0 (open   -> overwrites with tanh(1.0))
    // k=2: cand = tanh(0.5), gate bias =  0.0  (half   -> 0.5*h[2] + 0.5*tanh(0.5))
    // k=3: cand = tanh(-2.0), gate bias = 0.0  (half   -> 0.5*h[3] + 0.5*tanh(-2.0))
    params[l1.bias(ACTION_WIDTH).expect("b")] = 1.0;
    params[l1.bias(ACTION_WIDTH + 4).expect("b")] = -30.0;

    params[l1.bias(ACTION_WIDTH + 1).expect("b")] = 1.0;
    params[l1.bias(ACTION_WIDTH + 5).expect("b")] = 30.0;

    params[l1.bias(ACTION_WIDTH + 2).expect("b")] = 0.5;
    params[l1.bias(ACTION_WIDTH + 6).expect("b")] = 0.0;

    params[l1.bias(ACTION_WIDTH + 3).expect("b")] = -2.0;
    params[l1.bias(ACTION_WIDTH + 7).expect("b")] = 0.0;

    let ctrl = ReferenceController::new(&topo, &params, 50.0).expect("ctrl");
    let mut scratch = ControllerScratch::new(&topo).expect("scratch");
    let zeros_in = [0.0_f64; SENSOR_WIDTH];
    let state_a = [0.6_f32, -0.75, 0.4, 0.8];
    let mut next_a = [0.0_f32; 4];
    let out_a = ctrl
        .evaluate_recurrent(&zeros_in, &state_a, &mut scratch, &mut next_a)
        .expect("eval a");

    // Closed gate retains h[0] = 0.6; open gate sets tanh(1.0); half gates average.
    assert!((next_a[0] - 0.6).abs() < 1e-6, "closed gate: {}", next_a[0]);
    assert!(
        (next_a[1] - (1.0_f64.tanh() as f32)).abs() < 1e-6,
        "open gate: {}",
        next_a[1]
    );
    assert!(
        (next_a[2] - (0.5 * 0.4 + 0.5 * (0.5_f64.tanh() as f32))).abs() < 1e-6,
        "half gate 2: {}",
        next_a[2]
    );
    assert!(
        (next_a[3] - (0.5 * 0.8 + 0.5 * ((-2.0_f64).tanh() as f32))).abs() < 1e-6,
        "half gate 3: {}",
        next_a[3]
    );

    // Same sensor inputs with a different state change action[0] directly.
    let state_b = [-0.6_f32, -0.75, 0.4, 0.8];
    let mut next_b = [0.0_f32; 4];
    let out_b = ctrl
        .evaluate_recurrent(&zeros_in, &state_b, &mut scratch, &mut next_b)
        .expect("eval b");
    assert!(
        out_a[0] > 0.5 && out_b[0] < -0.5,
        "state must drive action: out_a={} out_b={}",
        out_a[0],
        out_b[0]
    );

    // 2. Bit-exact oracle comparison across 1-hidden and 2-hidden topologies in F32 and F16.
    for (hidden, s) in [(&[8_u32][..], 4_u32), (&[64, 32][..], 8_u32)] {
        let topo = DenseTopology::new_recurrent(hidden, s).expect("topo");
        let f32_params = pseudo_values(0x5eed_054 + u64::from(s), topo.parameter_count(), 0.4);
        let f16_params: Vec<F16> = f32_params
            .iter()
            .map(|&v| F16::from_f64(f64::from(v)))
            .collect();
        let mut inputs = [0.0_f64; SENSOR_WIDTH];
        for (i, slot) in inputs.iter_mut().enumerate() {
            *slot = ((i as f64 * 0.37).sin()) * 0.8;
        }
        let state = pseudo_values(0x1234 + u64::from(s), s as usize, 0.9);

        let ctrl_f32 = ReferenceController::new(&topo, &f32_params, 10.0).expect("f32 ctrl");
        let (exp_out, exp_state) =
            independent_recurrent_oracle(&topo, &f32_params, &inputs, &state);
        let mut got_state = vec![0.0_f32; s as usize];
        let mut scratch = ControllerScratch::new(&topo).expect("scratch");
        let got_out = ctrl_f32
            .evaluate_recurrent(&inputs, &state, &mut scratch, &mut got_state)
            .expect("eval f32");
        assert_eq!(got_out.map(f32::to_bits), exp_out.map(f32::to_bits));
        assert_eq!(
            got_state.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            exp_state.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );

        let trace = ctrl_f32
            .trace_recurrent(&inputs, &state, &mut scratch)
            .expect("trace f32");
        assert_eq!(trace.outputs.map(f32::to_bits), got_out.map(f32::to_bits));
        assert_eq!(
            trace
                .next_state
                .iter()
                .map(|v| v.to_bits())
                .collect::<Vec<_>>(),
            got_state.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
        assert_eq!(trace.hidden.len(), hidden.len());

        let ctrl_f16 = ReferenceController::new(&topo, &f16_params, 10.0).expect("f16 ctrl");
        let (exp_out_16, exp_state_16) =
            independent_recurrent_oracle(&topo, &f16_params, &inputs, &state);
        let mut got_state_16 = vec![0.0_f32; s as usize];
        let got_out_16 = ctrl_f16
            .evaluate_recurrent(&inputs, &state, &mut scratch, &mut got_state_16)
            .expect("eval f16");
        assert_eq!(got_out_16.map(f32::to_bits), exp_out_16.map(f32::to_bits));
        assert_eq!(
            got_state_16.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            exp_state_16.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
    }

    // 3. Long-horizon boundedness under extreme weights & inputs (200 steps).
    let max_topo =
        DenseTopology::new_recurrent(&[32], MAXIMUM_RECURRENT_STATE_WIDTH as u32).expect("max");
    let hot_params = pseudo_values(0xdead_beef, max_topo.parameter_count(), 10.0);
    let hot_ctrl = ReferenceController::new(&max_topo, &hot_params, 10.0).expect("hot");
    let mut scratch = ControllerScratch::new(&max_topo).expect("scratch");
    let mut h = vec![0.0_f32; MAXIMUM_RECURRENT_STATE_WIDTH];
    let mut h_next = vec![0.0_f32; MAXIMUM_RECURRENT_STATE_WIDTH];
    for step in 0..200_u32 {
        let mut x = [0.0_f64; SENSOR_WIDTH];
        for (i, slot) in x.iter_mut().enumerate() {
            *slot = if (i + step as usize) % 2 == 0 {
                1.0
            } else {
                -1.0
            };
        }
        let out = hot_ctrl
            .evaluate_recurrent(&x, &h, &mut scratch, &mut h_next)
            .expect("bounded step");
        assert!(out.iter().all(|v| v.is_finite()));
        assert!(
            h_next
                .iter()
                .all(|&v| v.is_finite() && (-1.0..=1.0).contains(&v)),
            "state escaped [-1, 1] at step {step}: {h_next:?}"
        );
        std::mem::swap(&mut h, &mut h_next);
    }

    // 4. Explicit refusals for mismatched evaluation modes and invalid states.
    assert!(matches!(
        ctrl.evaluate(&zeros_in, &mut scratch),
        Err(CoreError::LayoutMismatch { .. })
    ));
    let stateless_topo = DenseTopology::new(&[8]).expect("stateless");
    let stateless_params = vec![0.0_f32; stateless_topo.parameter_count()];
    let stateless_ctrl =
        ReferenceController::new(&stateless_topo, &stateless_params, 10.0).expect("stateless");
    assert!(matches!(
        stateless_ctrl.evaluate_recurrent(&zeros_in, &state_a, &mut scratch, &mut next_a),
        Err(CoreError::LayoutMismatch { .. })
    ));
    assert!(matches!(
        ctrl.evaluate_recurrent(&zeros_in, &[1.5, 0.0, 0.0, 0.0], &mut scratch, &mut next_a),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(matches!(
        ctrl.evaluate_recurrent(
            &zeros_in,
            &[f32::NAN, 0.0, 0.0, 0.0],
            &mut scratch,
            &mut next_a
        ),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(matches!(
        DenseTopology::new_recurrent(&[8], 0),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(matches!(
        DenseTopology::new_recurrent(&[8], MAXIMUM_RECURRENT_STATE_WIDTH as u32 + 1),
        Err(CoreError::NumericalInvalid { .. })
    ));
}
