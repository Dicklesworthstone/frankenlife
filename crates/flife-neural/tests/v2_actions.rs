//! V2-05 (`so2-motor`) and V2-06 (`so2-budding`) action layout and decoding
//! tests, plus V2-04 (`wake-offset` pump) and V2-08 (`emt-gate`) slot decoding.

#![allow(
    clippy::expect_used,
    reason = "test helpers: a failure in one is a test failure"
)]

use flife_core::numerics::{sigmoid, tanh};
use flife_core::{CoreError, TimeStep};
use flife_neural::{
    ACTION_WIDTH, ActionDecoding, ActionLayout, BuddingSteering, MotorSteering,
    SO2_BUDDING_OUTPUTS, SO2_MOTOR_OUTPUTS, decode, decode_slice, decode_with_layout,
};

fn test_decoding() -> ActionDecoding {
    ActionDecoding::new(0.8, [0.25, 0.75], 0.355, 0.5, 1.0e-6).expect("valid decoding constants")
}

fn test_step() -> TimeStep {
    TimeStep::new(1, 32).expect("1/32 s")
}

#[test]
fn action_layout_slot_accessors_match_canonical_abi_order() -> Result<(), CoreError> {
    for motor in [false, true] {
        for budding in [false, true] {
            for emt in [false, true] {
                for pump in [false, true] {
                    for state_width in [0_usize, 2, 16] {
                        let layout = ActionLayout::new(motor, budding, emt, pump, state_width)?;
                        let mut cursor = ACTION_WIDTH;

                        if motor {
                            assert_eq!(layout.so2_motor(), Some(cursor));
                            assert_eq!(
                                layout.motor_slots(),
                                Some(cursor..(cursor + SO2_MOTOR_OUTPUTS))
                            );
                            cursor += SO2_MOTOR_OUTPUTS;
                        } else {
                            assert_eq!(layout.so2_motor(), None);
                            assert_eq!(layout.motor_slots(), None);
                        }

                        if budding {
                            assert_eq!(layout.so2_budding(), Some(cursor));
                            assert_eq!(
                                layout.budding_slots(),
                                Some(cursor..(cursor + SO2_BUDDING_OUTPUTS))
                            );
                            cursor += SO2_BUDDING_OUTPUTS;
                        } else {
                            assert_eq!(layout.so2_budding(), None);
                            assert_eq!(layout.budding_slots(), None);
                        }

                        if emt {
                            assert_eq!(layout.emt_gate(), Some(cursor));
                            assert_eq!(layout.emt_slot(), Some(cursor));
                            cursor += 1;
                        } else {
                            assert_eq!(layout.emt_gate(), None);
                            assert_eq!(layout.emt_slot(), None);
                        }

                        if pump {
                            assert_eq!(layout.wake_pump(), Some(cursor));
                            assert_eq!(layout.pump_slot(), Some(cursor));
                            cursor += 1;
                        } else {
                            assert_eq!(layout.wake_pump(), None);
                            assert_eq!(layout.pump_slot(), None);
                        }

                        assert_eq!(layout.action_width(), cursor);
                        assert_eq!(layout.raw_output_width(), cursor + 2 * state_width);
                        assert_eq!(layout.recurrent_tail(), cursor..(cursor + 2 * state_width));
                    }
                }
            }
        }
    }
    Ok(())
}

#[test]
fn so2_motor_decodes_via_core_tanh_and_zero_is_exact() -> Result<(), CoreError> {
    let zero = MotorSteering::decode([0.0, -0.0, 0.0, -0.0])?;
    assert!(zero.is_zero());
    assert_eq!(zero.energy_gradient, 0.0);
    assert_eq!(zero.chemical_gradient, 0.0);
    assert_eq!(zero.chiral_contour, 0.0);
    assert_eq!(zero.heading_propulsion, 0.0);
    assert!(MotorSteering::ZERO.is_zero());

    let cases = [
        [-25.0, -1.25, 1.0e-12, 25.0],
        [-0.001, 0.5, -3.75, 19.0],
        [100.0, -100.0, 0.0, 0.25],
    ];
    for raw in cases {
        let got = MotorSteering::decode(raw)?;
        assert!(!got.is_zero());
        let want = [
            tanh(raw[0]).expect("finite"),
            tanh(raw[1]).expect("finite"),
            tanh(raw[2]).expect("finite"),
            tanh(raw[3]).expect("finite"),
        ];
        assert_eq!(got.to_array().map(f64::to_bits), want.map(f64::to_bits));
        for component in got.to_array() {
            assert!((-1.0..=1.0).contains(&component));
        }
    }

    for bad_idx in 0..SO2_MOTOR_OUTPUTS {
        for bad_val in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut raw = [0.1, -0.2, 0.3, -0.4];
            raw[bad_idx] = bad_val;
            assert!(matches!(
                MotorSteering::decode(raw),
                Err(CoreError::NumericalInvalid { .. })
            ));
        }
    }
    Ok(())
}

#[test]
fn so2_budding_decodes_and_clamps_unit_ball_with_so2_symmetry() -> Result<(), CoreError> {
    let zero = BuddingSteering::decode([0.0, -0.0])?;
    assert!(zero.is_zero());
    assert_eq!(zero.radial, 0.0);
    assert_eq!(zero.tangential, 0.0);
    assert!(BuddingSteering::ZERO.is_zero());

    // Small logits inside the unit disk remain exact tanh values untouched.
    let small = BuddingSteering::decode([0.3, -0.4])?;
    let w0 = tanh(0.3).expect("finite");
    let w1 = tanh(-0.4).expect("finite");
    assert!(w0 * w0 + w1 * w1 < 1.0);
    assert_eq!(small.radial.to_bits(), w0.to_bits());
    assert_eq!(small.tangential.to_bits(), w1.to_bits());

    // Saturating diagonal logits (tanh -> [1, 1], norm = sqrt(2) > 1) clamp
    // radially onto the exact left-fold unit disk at 45 degrees.
    let corner = BuddingSteering::decode([25.0, 25.0])?;
    let sq = corner.radial * corner.radial + corner.tangential * corner.tangential;
    assert!(sq <= 1.0, "left-fold squared norm {sq} exceeded 1.0");
    assert!(sq > 1.0 - 1.0e-14, "over-shrunk squared norm {sq}");
    assert_eq!(corner.radial.to_bits(), corner.tangential.to_bits());
    assert!((corner.radial - std::f64::consts::FRAC_1_SQRT_2).abs() <= 2.0 * f64::EPSILON);

    // Axis-aligned saturation reaches exact ±1.0 on the axis and 0.0 orthogonally.
    let axis = BuddingSteering::decode([-30.0, 0.0])?;
    assert_eq!(axis.radial.to_bits(), (-1.0_f64).to_bits());
    assert_eq!(axis.tangential.to_bits(), 0.0_f64.to_bits());

    // Exact sign reflection and component swap symmetry under clamping.
    for &(a, b) in &[(1.2, 2.4), (5.0, 1.5), (0.95, 0.95), (20.0, -3.0)] {
        let p = BuddingSteering::decode([a, b])?;
        let n = BuddingSteering::decode([-a, -b])?;
        let swap = BuddingSteering::decode([b, a])?;
        assert!(p.radial * p.radial + p.tangential * p.tangential <= 1.0);
        assert_eq!(n.radial.to_bits(), (-p.radial).to_bits());
        assert_eq!(n.tangential.to_bits(), (-p.tangential).to_bits());
        assert_eq!(swap.radial.to_bits(), p.tangential.to_bits());
        assert_eq!(swap.tangential.to_bits(), p.radial.to_bits());
    }

    for bad in [
        [f64::NAN, 0.0],
        [0.0, f64::INFINITY],
        [f64::NEG_INFINITY, 1.0],
    ] {
        assert!(matches!(
            BuddingSteering::decode(bad),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }
    Ok(())
}

#[test]
fn decode_with_layout_handles_all_v2_slots_and_preserves_v1_bit_exact() -> Result<(), CoreError> {
    let decoding = test_decoding();
    let step = test_step();

    let mut base = [0.0_f64; ACTION_WIDTH];
    base[0] = 0.7;
    base[1] = -0.35;
    base[2] = 1.4;
    base[3] = 0.6;
    base[4] = -0.8;
    base[35] = 0.9;

    let d_fixed = decode(&base, step, &decoding)?;
    let d_slice = decode_slice(&base, step, &decoding)?;
    let d_v1 = decode_with_layout(&base, step, &decoding, &ActionLayout::v1())?;
    assert_eq!(d_fixed, d_slice);
    assert_eq!(d_fixed, d_v1);
    assert_eq!(d_v1.motor, None);
    assert_eq!(d_v1.budding, None);
    assert_eq!(d_v1.emt_gate, None);
    assert_eq!(d_v1.wake_pump, None);

    // Bundle B: so2-motor (+4) + so2-budding (+2) -> 42 actions.
    let bundle_b = ActionLayout::new(true, true, false, false, 0)?;
    let mut raw_b = Vec::from(base);
    raw_b.extend_from_slice(&[0.5, -1.25, 2.0, -0.1, 1.8, -1.4]);
    let decoded_b = decode_with_layout(&raw_b, step, &decoding, &bundle_b)?;
    assert_eq!(
        decoded_b.attempt_rate.to_bits(),
        d_v1.attempt_rate.to_bits()
    );
    assert_eq!(
        decoded_b.attempt_probability.to_bits(),
        d_v1.attempt_probability.to_bits()
    );
    assert_eq!(
        decoded_b.child_fraction.to_bits(),
        d_v1.child_fraction.to_bits()
    );
    assert_eq!(
        decoded_b.emission_gate.to_bits(),
        d_v1.emission_gate.to_bits()
    );
    assert_eq!(decoded_b.emission, d_v1.emission);
    assert_eq!(
        decoded_b.motor,
        Some(MotorSteering::decode([0.5, -1.25, 2.0, -0.1])?)
    );
    assert_eq!(
        decoded_b.budding,
        Some(BuddingSteering::decode([1.8, -1.4])?)
    );
    assert_eq!(decoded_b.emt_gate, None);
    assert_eq!(decoded_b.wake_pump, None);

    // All-21 + S=2: motor(36..40), budding(40..42), emt(42), pump(43), recurrent tail(44..48).
    let all = ActionLayout::new(true, true, true, true, 2)?;
    let mut raw_all = raw_b.clone();
    raw_all.extend_from_slice(&[-0.75, 1.35, 0.2, -0.4, 0.1, -0.9]);
    assert_eq!(raw_all.len(), all.raw_output_width());
    // Both full raw outputs (48) and action-only slice (44) decode identically.
    let from_full = decode_with_layout(&raw_all, step, &decoding, &all)?;
    let from_actions = decode_with_layout(&raw_all[..all.action_width()], step, &decoding, &all)?;
    assert_eq!(from_full, from_actions);
    assert_eq!(from_full.motor, decoded_b.motor);
    assert_eq!(from_full.budding, decoded_b.budding);
    assert_eq!(
        from_full.emt_gate.map(f64::to_bits),
        sigmoid(-0.75).map(f64::to_bits)
    );
    assert_eq!(
        from_full.wake_pump.map(f64::to_bits),
        tanh(1.35).map(f64::to_bits)
    );

    // Budding-only (without motor): budding slots shift to 36..38 (never hardcoded!).
    let budding_only = ActionLayout::new(false, true, false, false, 0)?;
    assert_eq!(budding_only.budding_slots(), Some(36..38));
    let mut raw_bud = Vec::from(base);
    raw_bud.extend_from_slice(&[0.6, -0.9]);
    let decoded_bud = decode_with_layout(&raw_bud, step, &decoding, &budding_only)?;
    assert_eq!(decoded_bud.motor, None);
    assert_eq!(
        decoded_bud.budding,
        Some(BuddingSteering::decode([0.6, -0.9])?)
    );
    Ok(())
}

#[test]
fn decode_with_layout_refuses_short_slice_and_nonfinite_values() -> Result<(), CoreError> {
    let decoding = test_decoding();
    let step = test_step();
    let all = ActionLayout::new(true, true, true, true, 2)?;

    let short = vec![0.0_f64; all.action_width() - 1];
    assert!(matches!(
        decode_with_layout(&short, step, &decoding, &all),
        Err(CoreError::LayoutMismatch { .. })
    ));

    for bad_idx in [0, 35, 36, 39, 40, 41, 42, 43, 47] {
        let mut raw = vec![0.0_f64; all.raw_output_width()];
        raw[bad_idx] = f64::NAN;
        assert!(
            matches!(
                decode_with_layout(&raw, step, &decoding, &all),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "expected NumericalInvalid for NaN at index {bad_idx}"
        );
    }
    Ok(())
}
