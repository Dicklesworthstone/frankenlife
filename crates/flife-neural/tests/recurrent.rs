//! FL-054: independent known-answer, bounded-execution, and schema tests for
//! `independent-recurrent-mlp-v1` (plan §6.2, §17.5, §25.2).

#![allow(
    clippy::expect_used,
    reason = "test helpers: a failure in one is a test failure"
)]

use flife_core::binary16::F16;
use flife_core::config::{ContactConfig, RunConfig};
use flife_core::model::FIELD_ECOLOGY_V2;
use flife_core::numerics::{sigmoid, symplectic_rotate_channels, tanh};
use flife_core::v2::V2Config;
use flife_core::{CoreError, ValidatedConfig, validate};
use flife_neural::{
    ACTION_WIDTH, ActionLayout, ControllerScratch, DenseTopology, INDEPENDENT_RECURRENT_MLP_V1,
    MAX_ACTION_WIDTH, MAX_CONTROLLER_OUTPUT_WIDTH, MAX_SENSOR_WIDTH, MAXIMUM_RECURRENT_STATE_WIDTH,
    RawOutputs, ReferenceController, SENSOR_WIDTH, SensorLayout, Weight,
    rotate_epigenetic_affinity, update_recurrent_state,
};

const STARTER_TOML: &str = include_str!("../../../presets/reference-small-world.toml");

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
        let f32_params = pseudo_values(0x05ee_d054 + u64::from(s), topo.parameter_count(), 0.4);
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
            *slot = if (i + step as usize).is_multiple_of(2) {
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

/// V2-01: canonical pure-scientific `SensorLayout` & `ActionLayout` index
/// allocation, alignment properties, `V2Abi` agreement, `DenseTopology::from_layouts`,
/// and shared `update_recurrent_state` (including slow epigenetic gate bias `b_epi`).
#[test]
fn v2_layouts_and_shared_recurrent_state_update_are_canonical() {
    assert_eq!(MAX_SENSOR_WIDTH, 128);
    assert_eq!(MAX_ACTION_WIDTH, 48);
    assert_eq!(MAX_CONTROLLER_OUTPUT_WIDTH, 80);

    // 1. Baseline v1 (104/36).
    let v1_s = SensorLayout::v1();
    let v1_a = ActionLayout::v1();
    assert_eq!(
        (
            v1_s.observation_width(),
            v1_s.recurrent_state_width(),
            v1_s.input_width()
        ),
        (104, 0, 104)
    );
    assert_eq!(
        (
            v1_a.action_width(),
            v1_a.recurrent_state_width(),
            v1_a.raw_output_width()
        ),
        (36, 0, 36)
    );
    assert!(v1_s.is_aligned4());
    assert_eq!(v1_s.dual_carrier_trails(), None);
    assert_eq!(v1_s.hippo_crowding(), None);
    assert_eq!(v1_s.notch_delta(), None);
    assert_eq!(v1_s.recurrent_state(), None);
    assert_eq!(v1_a.so2_motor(), None);
    assert_eq!(v1_a.so2_budding(), None);
    assert_eq!(v1_a.emt_gate(), None);
    assert_eq!(v1_a.wake_pump(), None);

    // 2. Bundle A (Animal Tissue: hippo + notch + S=2 -> 108; so2-motor + S=2 -> 40/44).
    let bundle_a_s = SensorLayout::new(false, true, true, 2).expect("bundle A sensors");
    let bundle_a_a = ActionLayout::new(true, false, false, false, 2).expect("bundle A actions");
    assert_eq!(bundle_a_s.input_width(), 108);
    assert!(bundle_a_s.is_aligned4());
    assert_eq!(bundle_a_s.hippo_crowding(), Some(104));
    assert_eq!(bundle_a_s.notch_delta(), Some(105));
    assert_eq!(bundle_a_s.recurrent_state(), Some(106));
    assert_eq!(bundle_a_s.recurrent_state_range(), 106..108);
    assert_eq!(bundle_a_a.action_width(), 40);
    assert_eq!(bundle_a_a.raw_output_width(), 44);
    assert_eq!(bundle_a_a.so2_motor(), Some(36));
    assert_eq!(bundle_a_a.recurrent_candidates(), 40..42);
    assert_eq!(bundle_a_a.recurrent_gates(), 42..44);
    assert_eq!(bundle_a_a.recurrent_tail(), 40..44);

    // 3. Bundle B (Super-Colony: dual-carrier-trails -> 108; so2-motor + so2-budding -> 42).
    let bundle_b_s = SensorLayout::new(true, false, false, 0).expect("bundle B sensors");
    let bundle_b_a = ActionLayout::new(true, true, false, false, 0).expect("bundle B actions");
    assert_eq!(bundle_b_s.input_width(), 108);
    assert_eq!(bundle_b_s.dual_carrier_trails(), Some(104));
    assert_eq!(bundle_b_a.action_width(), 42);
    assert_eq!(bundle_b_a.so2_motor(), Some(36));
    assert_eq!(bundle_b_a.so2_budding(), Some(40));

    // 4. All-21 (S=2): 104 + 4 + 1 + 1 + 2 = 112 sensors; 36 + 4 + 2 + 1 + 1 = 44 actions, 48 raw.
    let all_s = SensorLayout::new(true, true, true, 2).expect("all sensors");
    let all_a = ActionLayout::new(true, true, true, true, 2).expect("all actions");
    assert_eq!(all_s.input_width(), 112);
    assert!(all_s.is_aligned4());
    assert_eq!(all_s.dual_carrier_trails(), Some(104));
    assert_eq!(all_s.hippo_crowding(), Some(108));
    assert_eq!(all_s.notch_delta(), Some(109));
    assert_eq!(all_s.recurrent_state(), Some(110));
    assert_eq!(all_a.action_width(), 44);
    assert_eq!(all_a.raw_output_width(), 48);
    assert_eq!(all_a.so2_motor(), Some(36));
    assert_eq!(all_a.so2_budding(), Some(40));
    assert_eq!(all_a.emt_gate(), Some(42));
    assert_eq!(all_a.wake_pump(), Some(43));
    assert_eq!(all_a.recurrent_candidates(), 44..46);
    assert_eq!(all_a.recurrent_gates(), 46..48);

    // 5. Custom odd subset (105 sensors, 37 outputs): pure scientific, NO silent padding!
    let odd_s = SensorLayout::new(false, true, false, 0).expect("odd sensors");
    let odd_a = ActionLayout::new(false, false, true, false, 0).expect("odd actions");
    assert_eq!(odd_s.input_width(), 105);
    assert!(!odd_s.is_aligned4());
    assert_eq!(odd_a.action_width(), 37);
    assert_eq!(odd_a.raw_output_width(), 37);

    // 6. DenseTopology::from_layouts & V2Abi round-trip.
    let topo = DenseTopology::from_layouts(&bundle_a_s, &[64, 32], &bundle_a_a).expect("topo");
    assert_eq!(topo.sensor_width(), 106);
    assert_eq!(topo.input_width(), 108);
    assert_eq!(topo.action_width(), 40);
    assert_eq!(topo.raw_output_width(), 44);
    assert_eq!(topo.recurrent_state_width(), 2);
    assert_eq!(
        topo.parameter_count(),
        (108 * 64 + 64) + (64 * 32 + 32) + (32 * 44 + 44)
    );
    assert!(matches!(
        DenseTopology::from_layouts(&bundle_a_s, &[8], &v1_a),
        Err(CoreError::LayoutMismatch { .. })
    ));

    // 7. Shared `update_recurrent_state` matches independent formula for both
    //    `gate_bias = 0.0` (FL-054) and positive/negative `gate_bias` (`-b_epi`, V2-12).
    let state = [0.6_f32, -0.5, 0.25];
    let raw_tail = [0.8_f32, -1.25, 0.0, -0.5, 1.0, 0.25];
    for bias in [0.0_f64, 2.5, -2.5] {
        let mut next = [0.0_f32; 3];
        update_recurrent_state(&state, &raw_tail, bias, &mut next).expect("update");
        for k in 0..3 {
            let cand = tanh(f64::from(raw_tail[k])).expect("tanh") as f32;
            let gate_arg = if bias == 0.0 {
                f64::from(raw_tail[3 + k])
            } else {
                f64::from(raw_tail[3 + k]) - bias
            };
            let gate = sigmoid(gate_arg).expect("sigmoid") as f32;
            let expected = ((1.0_f32 - gate) * state[k] + gate * cand).clamp(-1.0_f32, 1.0_f32);
            assert_eq!(next[k].to_bits(), expected.to_bits(), "bias={bias} k={k}");
        }
    }

    // Verify `evaluate_raw` + `update_recurrent_state` == `evaluate_recurrent_slice` bit-for-bit.
    let params = pseudo_values(0x5632_0001, topo.parameter_count(), 0.35);
    let ctrl = ReferenceController::new(&topo, &params, 10.0).expect("ctrl");
    let mut scratch = ControllerScratch::new(&topo).expect("scratch");
    let sensor_in: Vec<f64> = (0..106).map(|i| ((i as f64) * 0.21).sin()).collect();
    let h0 = [0.4_f32, -0.7];
    let mut full_in = sensor_in.clone();
    full_in.extend(h0.iter().map(|&v| f64::from(v)));

    let raw: RawOutputs = ctrl.evaluate_raw(&full_in, &mut scratch).expect("raw");
    assert_eq!(raw.len(), 44);
    let mut via_raw_next = [0.0_f32; 2];
    update_recurrent_state(&h0, &raw[40..44], 2.5, &mut via_raw_next).expect("update");

    let mut via_slice_next = [0.0_f32; 2];
    let slice_actions = ctrl
        .evaluate_recurrent_slice(&sensor_in, &h0, 2.5, &mut scratch, &mut via_slice_next)
        .expect("recurrent slice");
    assert_eq!(slice_actions.len(), 40);
    assert_eq!(
        raw[..40].iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        slice_actions
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        via_raw_next.map(f32::to_bits),
        via_slice_next.map(f32::to_bits)
    );

    // Refusals for `update_recurrent_state`.
    let mut bad_next = [0.0_f32; 2];
    assert!(matches!(
        update_recurrent_state(&h0, &raw[40..44], f64::NAN, &mut bad_next),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(matches!(
        update_recurrent_state(&[1.2, 0.0], &raw[40..44], 0.0, &mut bad_next),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(matches!(
        update_recurrent_state(&h0, &raw[40..43], 0.0, &mut bad_next),
        Err(CoreError::LayoutMismatch { .. })
    ));
}

fn starter_config() -> RunConfig {
    RunConfig::from_toml(STARTER_TOML).expect("reference-small-world preset parses")
}

fn sample_contact() -> ContactConfig {
    ContactConfig {
        diameter: 0.25,
        stiffness: 0.5,
        adhesion: 0.2,
        adhesion_range: 1.5,
        sweeps: None,
        inhibition: None,
        repulsion: None,
    }
}

/// Real `ValidatedConfig` -> `SensorLayout::from_config` / `ActionLayout::from_config`
/// / `DenseTopology::from_config` integration across v1, v1-recurrent, and all
/// v2 profiles (`minimal`, `supercolony`, `tissue`, `all`).
#[test]
fn v2_validated_configs_resolve_sensor_action_layouts_and_topology() {
    let check = |cfg: &ValidatedConfig, want_obs: usize, want_state: usize, want_actions: usize| {
        let abi = cfg.v2_abi();
        let sensors = SensorLayout::from_config(cfg).expect("sensor layout");
        let actions = ActionLayout::from_config(cfg).expect("action layout");
        let topo = DenseTopology::from_config(cfg).expect("topology");

        assert_eq!(sensors.observation_width(), want_obs);
        assert_eq!(sensors.recurrent_state_width(), want_state);
        assert_eq!(sensors.input_width(), want_obs + want_state);
        assert_eq!(actions.action_width(), want_actions);
        assert_eq!(actions.recurrent_state_width(), want_state);
        assert_eq!(actions.raw_output_width(), want_actions + 2 * want_state);

        assert_eq!(sensors.observation_width(), abi.sensor_width as usize);
        assert_eq!(sensors.input_width(), abi.input_width as usize);
        assert_eq!(actions.action_width(), abi.action_width as usize);
        assert_eq!(actions.raw_output_width(), abi.raw_output_width as usize);

        assert_eq!(topo.sensor_width(), want_obs);
        assert_eq!(topo.input_width(), want_obs + want_state);
        assert_eq!(topo.action_width(), want_actions);
        assert_eq!(topo.raw_output_width(), want_actions + 2 * want_state);
        assert_eq!(topo.recurrent_state_width(), want_state);
    };

    // 1. Baseline field-ecology-v1 (104/36).
    let v1 = validate(starter_config()).expect("v1 validates");
    check(&v1, 104, 0, 36);

    // 2. field-ecology-v1 with independent-recurrent-mlp-v1 (S=4 -> 108/44).
    let mut v1_rec = starter_config();
    v1_rec.controller.family = INDEPENDENT_RECURRENT_MLP_V1.to_owned();
    v1_rec.controller.recurrent_state_width = Some(4);
    v1_rec.architecture_mutation.catalog = "recurrent-104-36-catalog-v1".to_owned();
    let v1_rec = validate(v1_rec).expect("v1 recurrent validates");
    check(&v1_rec, 104, 4, 36);

    // 3. field-ecology-v2 minimal (v1 compat -> 104/36).
    let mut minimal = starter_config();
    minimal.model = FIELD_ECOLOGY_V2.to_owned();
    minimal.v2 = Some(V2Config {
        profile: "minimal".to_owned(),
        ..V2Config::default()
    });
    let minimal = validate(minimal).expect("v2 minimal validates");
    check(&minimal, 104, 0, 36);

    // 4. field-ecology-v2 supercolony / Bundle B (dual-carrier +4, motor +4, budding +2 -> 108/42).
    let mut bundle_b = starter_config();
    bundle_b.model = FIELD_ECOLOGY_V2.to_owned();
    let bundle_b = validate(bundle_b).expect("v2 supercolony validates");
    check(&bundle_b, 108, 0, 42);

    // 5. field-ecology-v2 tissue / Bundle A default (notch +1, S=2 -> 107; motor +4 -> 40/44).
    let mut tissue = starter_config();
    tissue.model = FIELD_ECOLOGY_V2.to_owned();
    tissue.contact = Some(sample_contact());
    let tissue = validate(tissue).expect("v2 tissue validates");
    check(&tissue, 105, 2, 40);

    // 6. field-ecology-v2 tissue with hippo expose_sensor (108/40/44).
    let mut tissue_108 = starter_config();
    tissue_108.model = FIELD_ECOLOGY_V2.to_owned();
    tissue_108.contact = Some(sample_contact());
    let mut v2_t = V2Config::default();
    v2_t.crowding.expose_sensor = true;
    tissue_108.v2 = Some(v2_t);
    let tissue_108 = validate(tissue_108).expect("v2 tissue 108/44 validates");
    check(&tissue_108, 106, 2, 40);

    // 7. field-ecology-v2 all-21 + expose_sensor + neural wake pump (112/44/48).
    let mut all_cfg = starter_config();
    all_cfg.model = FIELD_ECOLOGY_V2.to_owned();
    all_cfg.contact = Some(sample_contact());
    let mut v2_all = V2Config {
        profile: "all".to_owned(),
        ..V2Config::default()
    };
    v2_all.crowding.expose_sensor = true;
    v2_all.emission.offset.mode = "polarized-pump-v1".to_owned();
    v2_all.emission.offset.pump_direction_mode = "neural-output-v1".to_owned();
    all_cfg.v2 = Some(v2_all);
    let all_cfg = validate(all_cfg).expect("v2 all-21 validates");
    check(&all_cfg, 110, 2, 44);
}

/// V2-12 (`epigenetic-recurrence`): verify `-b_epi` sign convention
/// (`b_epi = 2.5` at `z_g = 0` yields `g = σ(-2.5) ≈ 0.075858`, ~13-tick
/// retention), monotonic slow-down as `b_epi` increases, `b_epi = 0.0`
/// bit-exactness against `v1` `FL-054` `evaluate_recurrent` (F32 & F16), and
/// finite/bounds/length refusals.
#[test]
fn recurrent_epigenetic_gate_bias_unit() {
    // 1. Analytic check at z_g = 0, state = 0.0, z_c = 20.0 (tanh(20) == 1.0):
    //    next_state = (1 - g) * 0 + g * 1 = g = sigmoid(-b_epi) as f32.
    let state = [0.0_f32, 0.8];
    let raw_tail = [20.0_f32, -20.0, 0.0, 0.0];
    let mut next_epi = [0.0_f32; 2];
    update_recurrent_state(&state, &raw_tail, 2.5, &mut next_epi).expect("b_epi=2.5");
    let expected_g = (1.0_f64 / (1.0_f64 + 2.5_f64.exp())) as f32; // ≈ 0.07585818
    assert!(
        (next_epi[0] - 0.075_858_18_f32).abs() < 1.0e-6,
        "b_epi=2.5 at z_g=0 must give g ~= 0.075858, got {}",
        next_epi[0]
    );
    assert_eq!(
        next_epi[0].to_bits(),
        (sigmoid(-2.5).expect("sigmoid(-2.5)") as f32).to_bits()
    );
    let expected_k1 = ((1.0_f32 - expected_g) * 0.8 - expected_g).clamp(-1.0, 1.0);
    assert!(
        (next_epi[1] - expected_k1).abs() < 1.0e-6 && next_epi[1] > 0.65,
        "slow epigenetic gate must retain most of state[1]=0.8, got {}",
        next_epi[1]
    );

    // 2. Monotonic slow-down: higher b_epi -> smaller update gate g -> smaller step |h' - h|.
    let mut next_zero = [0.0_f32; 2];
    let mut next_high = [0.0_f32; 2];
    update_recurrent_state(&state, &raw_tail, 0.0, &mut next_zero).expect("b_epi=0.0");
    update_recurrent_state(&state, &raw_tail, 4.0, &mut next_high).expect("b_epi=4.0");
    assert!((next_zero[0] - 0.5_f32).abs() < 1.0e-6);
    assert!(
        next_high[0] < next_epi[0] && next_epi[0] < next_zero[0],
        "expected g(4.0) < g(2.5) < g(0.0): {} vs {} vs {}",
        next_high[0],
        next_epi[0],
        next_zero[0]
    );

    // 3. b_epi = 0.0 matches v1 FL-054 `evaluate_recurrent` bit-for-bit across F32 & F16.
    let topo = DenseTopology::new_recurrent(&[32, 16], 4).expect("v1 recurrent");
    let f32_params = pseudo_values(0x2026_1201, topo.parameter_count(), 0.35);
    let f16_params: Vec<F16> = f32_params
        .iter()
        .map(|&v| F16::from_f64(f64::from(v)))
        .collect();
    let mut inputs = [0.0_f64; SENSOR_WIDTH];
    for (i, slot) in inputs.iter_mut().enumerate() {
        *slot = ((i as f64) * 0.29).cos() * 1.1;
    }
    let h0 = [0.75_f32, -0.4, 0.125, -0.9];
    let mut full_in = Vec::with_capacity(SENSOR_WIDTH + 4);
    full_in.extend_from_slice(&inputs);
    full_in.extend(h0.iter().map(|&v| f64::from(v)));

    let mut scratch = ControllerScratch::new(&topo).expect("scratch");
    for (label, ctrl) in [
        (
            "F32",
            ReferenceController::new(&topo, &f32_params, 10.0).expect("f32"),
        ),
        (
            "F16",
            ReferenceController::new(&topo, &f16_params, 10.0).expect("f16"),
        ),
    ] {
        let mut v1_next = [0.0_f32; 4];
        let v1_out = ctrl
            .evaluate_recurrent(&inputs, &h0, &mut scratch, &mut v1_next)
            .expect("v1 eval");
        let raw = ctrl.evaluate_raw(&full_in, &mut scratch).expect("raw eval");
        let mut shared_next = [0.0_f32; 4];
        update_recurrent_state(&h0, &raw[ACTION_WIDTH..], 0.0, &mut shared_next).expect("shared");
        assert_eq!(
            raw[..ACTION_WIDTH]
                .iter()
                .map(|v| v.to_bits())
                .collect::<Vec<_>>(),
            v1_out.map(f32::to_bits).to_vec(),
            "{label}: action bits"
        );
        assert_eq!(
            shared_next.map(f32::to_bits),
            v1_next.map(f32::to_bits),
            "{label}: b_epi=0.0 next_state must match v1 FL-054 bit-for-bit"
        );
    }

    // 4. Finite, range, and layout refusals.
    let mut out = [0.0_f32; 2];
    for bad_bias in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(matches!(
            update_recurrent_state(&state, &raw_tail, bad_bias, &mut out),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }
    for bad_tail in [
        [f32::NAN, 0.0, 0.0, 0.0],
        [f32::INFINITY, 0.0, 0.0, 0.0],
        [0.0, 0.0, f32::NAN, 0.0],
        [0.0, 0.0, f32::NEG_INFINITY, 0.0],
    ] {
        assert!(matches!(
            update_recurrent_state(&state, &bad_tail, 2.5, &mut out),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }
    assert!(matches!(
        update_recurrent_state(&[f32::NAN, 0.0], &raw_tail, 2.5, &mut out),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(matches!(
        update_recurrent_state(&[-1.0001, 0.0], &raw_tail, 2.5, &mut out),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(matches!(
        update_recurrent_state(&[0.0; 17], &[0.0; 34], 2.5, &mut [0.0; 17]),
        Err(CoreError::LayoutMismatch { .. })
    ));
}

/// V2-12 (`epigenetic-recurrence` metabolic caste rotation): sweeps 101 values
/// of `h_0 ∈ [-1.0, 1.0]` plus explicit negative boundary values (`-1.0`,
/// `-0.5`, `-1e-12`, `-0.0`, `0.0`, `1e-12`, `0.5`, `1.0`) over adversarial
/// unit vectors (`active_channels = 32` single-carrier and `24` Carrier-0),
/// verifying exact quarter-turns `a_eff(0) == a`, `a_eff(1) == J a`,
/// `a_eff(-1) == -J a`, exact left-fold `‖a_eff‖₂² <= limit²`, and refusals.
#[test]
fn a_eff_orthogonal_rotation_full_signed_range_left_fold_safe() {
    let left_fold_sq = |v: &[f64; 32]| v.iter().fold(0.0_f64, |t, &c| t + c * c);
    let dot = |a: &[f64; 32], b: &[f64; 32]| a.iter().zip(b).fold(0.0_f64, |t, (&x, &y)| t + x * y);

    // Build adversarial unit vectors right at the left-fold norm boundary 1.0.
    let mut vecs: Vec<[f64; 32]> = Vec::new();
    let mut equal = [1.0_f64 / 32.0_f64.sqrt(); 32];
    flife_core::numerics::shrink_to_left_fold_norm_limit(&mut equal, 1.0).expect("equal");
    vecs.push(equal);
    for seed in [0x1111_u64, 0x2222, 0x3333, 0x4444] {
        let raw = pseudo_values(seed, 32, 1.0);
        let mut a = [0.0_f64; 32];
        for (dst, &src) in a.iter_mut().zip(&raw) {
            *dst = f64::from(src);
        }
        let n = left_fold_sq(&a).sqrt();
        for c in &mut a {
            *c /= n;
        }
        flife_core::numerics::shrink_to_left_fold_norm_limit(&mut a, 1.0).expect("norm");
        vecs.push(a);
    }

    let mut samples: Vec<f32> = (0..=100).map(|i| (i as f32 - 50.0) / 50.0).collect();
    samples.extend([-1.0_f32, -0.5, -1.0e-12, -0.0, 0.0, 1.0e-12, 0.5, 1.0]);

    // 1. Single-carrier 32-channel rotation.
    for a in &vecs {
        let ja = symplectic_rotate_channels(a, 32, 1.0).expect("J a");
        assert!(dot(a, &ja).abs() < 1.0e-14, "a and J a must be orthogonal");

        let at_zero = rotate_epigenetic_affinity(a, 0.0, 32, 1.0).expect("h0=0");
        let at_pos1 = rotate_epigenetic_affinity(a, 1.0, 32, 1.0).expect("h0=+1");
        let at_neg1 = rotate_epigenetic_affinity(a, -1.0, 32, 1.0).expect("h0=-1");
        assert_eq!(at_zero, *a, "a_eff(0) == a");
        assert_eq!(at_pos1, ja, "a_eff(+1) == J a");
        assert_eq!(at_neg1, ja.map(|v| -v), "a_eff(-1) == -J a");

        for &h0 in &samples {
            let eff = rotate_epigenetic_affinity(a, h0, 32, 1.0).expect("h0 valid");
            assert!(eff.iter().all(|v| v.is_finite()), "h0={h0}");
            let sq = left_fold_sq(&eff);
            assert!(
                sq <= 1.0_f64,
                "left-fold ||a_eff({h0})||^2 = {sq:?} exceeded 1.0"
            );
            assert!(
                (sq - left_fold_sq(a)).abs() < 1.0e-12,
                "rotation must preserve norm at h0={h0}: {sq} vs {}",
                left_fold_sq(a)
            );
        }
    }

    // 2. Dual-carrier Carrier-0 (24 active channels) preserves Carrier-1 channels 24..32
    //    and satisfies left-fold norm <= 1.0 across the full signed range.
    let a_dual = &vecs[1];
    let ja_24 = symplectic_rotate_channels(a_dual, 24, 1.0).expect("J_24 a");
    for &h0 in &samples {
        let eff = rotate_epigenetic_affinity(a_dual, h0, 24, 1.0).expect("dual h0");
        assert!(left_fold_sq(&eff) <= 1.0_f64);
        assert!(eff[24..].iter().all(|v| v.is_finite()));
    }
    let eff_pos1 = rotate_epigenetic_affinity(a_dual, 1.0, 24, 1.0).expect("dual +1");
    for k in 0..24 {
        assert!((eff_pos1[k] - ja_24[k]).abs() < 1.0e-14);
    }
    for k in 24..32 {
        assert!((eff_pos1[k] - a_dual[k]).abs() < 1.0e-14);
    }

    // 3. Explicit refusals.
    for bad_h0 in [
        -1.0001_f32,
        1.0001,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ] {
        assert!(matches!(
            rotate_epigenetic_affinity(&vecs[0], bad_h0, 32, 1.0),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }
    assert!(matches!(
        rotate_epigenetic_affinity(&vecs[0], 0.0, 33, 1.0),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(matches!(
        rotate_epigenetic_affinity(&vecs[0], 0.0, 32, 0.0),
        Err(CoreError::NumericalInvalid { .. })
    ));
}
