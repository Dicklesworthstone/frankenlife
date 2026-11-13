//! The compact-F16 profile's binary16 values (plan §10.3): widening is
//! exact for all 65,536 bit patterns, checked against an independent
//! decoding, and the declared rounding from F64 is to nearest with ties to
//! even, checked against the exact midpoints between neighbouring values.

use flife_core::binary16::F16;

/// 2^n as F64, exactly (n within F64's normal range).
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn pow2(n: i32) -> f64 {
    f64::from_bits(u64::try_from(n + 1023).expect("a normal exponent") << 52)
}

/// The value of binary16 `bits` by the textbook formula, in F64 (NaN for
/// every NaN pattern).
fn decoded(bits: u16) -> f64 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = i32::from((bits >> 10) & 0x1F);
    let fraction = f64::from(bits & 0x3FF);
    match exponent {
        0 => sign * fraction * pow2(-24),
        0x1F if fraction == 0.0 => sign * f64::INFINITY,
        0x1F => f64::NAN,
        _ => sign * (1024.0 + fraction) * pow2(exponent - 25),
    }
}

/// The next F64 above a positive finite `x`, and the one below.
fn up(x: f64) -> f64 {
    f64::from_bits(x.to_bits() + 1)
}
fn down(x: f64) -> f64 {
    f64::from_bits(x.to_bits() - 1)
}

#[test]
fn every_bit_pattern_widens_exactly_and_rounds_back_to_itself() {
    let mut nans = 0;
    for bits in 0..=u16::MAX {
        let value = F16::from_bits(bits);
        assert_eq!(value.to_bits(), bits);
        let expected = decoded(bits);
        if expected.is_nan() {
            nans += 1;
            assert!(value.is_nan(), "{bits:#06x}");
            assert!(value.to_f32().is_nan() && value.to_f64().is_nan());
            assert_eq!(
                value.to_f32().is_sign_negative(),
                bits & 0x8000 != 0,
                "{bits:#06x} keeps its sign"
            );
            let back = F16::from_f64(value.to_f64());
            assert!(back.is_nan());
            assert_eq!(back.to_bits() & 0x8000, bits & 0x8000);
            continue;
        }
        assert!(!value.is_nan(), "{bits:#06x}");
        // Bit-exact, so the sign of zero counts.
        assert_eq!(value.to_f64().to_bits(), expected.to_bits(), "{bits:#06x}");
        assert_eq!(
            f64::from(value.to_f32()).to_bits(),
            expected.to_bits(),
            "{bits:#06x}"
        );
        assert_eq!(F16::from_f64(expected), value, "{bits:#06x} round trip");
    }
    // 2 signs × 1,023 nonzero payloads.
    assert_eq!(nans, 2046);
}

#[test]
fn rounding_goes_to_the_nearest_value_and_ties_to_even() {
    // Each pair of neighbouring finite non-negative values, and the
    // largest finite value with the binade boundary 65,536 above it.
    for low in 0_u16..=0x7BFF {
        let a = decoded(low);
        let b = if low == 0x7BFF {
            65_536.0
        } else {
            decoded(low + 1)
        };
        let high = low + 1; // 0x7C00, infinity, above the largest finite
        let mid = (a + b) / 2.0; // exact: one bit more than a and b need
        let even = if low & 1 == 0 { low } else { high };
        let cases = [
            (a, low),
            (a + (b - a) / 4.0, low),
            (down(mid), low),
            (mid, even),
            (up(mid), high),
            (b - (b - a) / 4.0, high),
        ];
        for (x, expected) in cases {
            assert_eq!(
                F16::from_f64(x).to_bits(),
                expected,
                "{x:e} after {low:#06x}"
            );
            assert_eq!(
                F16::from_f64(-x).to_bits(),
                expected | 0x8000,
                "{:e} after {low:#06x}",
                -x
            );
        }
    }
    // Overflow and the ends of F64.
    for (x, expected) in [
        (65_520.0, 0x7C00),
        (down(65_520.0), 0x7BFF),
        (1.0e10, 0x7C00),
        (f64::MAX, 0x7C00),
        (f64::INFINITY, 0x7C00),
        (pow2(-25), 0x0000),
        (up(pow2(-25)), 0x0001),
        (f64::MIN_POSITIVE, 0x0000),
        (f64::from_bits(1), 0x0000),
        (0.0, 0x0000),
    ] {
        assert_eq!(F16::from_f64(x).to_bits(), expected, "{x:e}");
        assert_eq!(F16::from_f64(-x).to_bits(), expected | 0x8000, "{:e}", -x);
    }
    assert_eq!(F16::from_f64(f64::NAN).to_bits() & 0x7FFF, 0x7E00);
}

/// splitmix64: a fixed, seeded source of bits for the sweep below.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[test]
fn a_million_values_round_into_their_midpoint_interval() {
    let mut state = 0x0F16_0F16_0F16_0F16_u64;
    for _ in 0..1_000_000 {
        let word = next(&mut state);
        // A binade from 2^-30 to 2^16 and a random 52-bit fraction.
        let exponent = i32::try_from(word % 47).expect("small") - 30;
        let x =
            f64::from_bits((word >> 12) | (u64::try_from(exponent + 1023).expect("normal") << 52));
        let rounded = F16::from_f64(x).to_bits();
        if x >= 65_520.0 {
            assert_eq!(rounded, 0x7C00, "{x:e}");
            continue;
        }
        assert!(rounded <= 0x7BFF, "{x:e} -> {rounded:#06x}");
        let value = decoded(rounded);
        let below = if rounded == 0 {
            0.0
        } else {
            (decoded(rounded - 1) + value) / 2.0
        };
        let above = if rounded == 0x7BFF {
            65_520.0
        } else {
            (value + decoded(rounded + 1)) / 2.0
        };
        assert!(below <= x && x <= above, "{x:e} -> {rounded:#06x}");
        if x == below || x == above {
            assert_eq!(rounded & 1, 0, "a tie at {x:e} goes to the even value");
        }
    }
}

#[test]
fn rounding_is_once_from_f64_not_through_f32() {
    // Just above the midpoint between 1 (even) and 1 + 2^-10 (odd), by
    // less than an F32 step: F32 would land on the midpoint itself, which
    // then ties to the even 1.
    let x = 1.0 + pow2(-11) + pow2(-40);
    assert_eq!(F16::from_f64(x).to_bits(), 0x3C01);
    #[expect(clippy::cast_possible_truncation, reason = "the double rounding shown")]
    let via_f32 = F16::from_f64(f64::from(x as f32));
    assert_eq!(via_f32.to_bits(), 0x3C00);
}
