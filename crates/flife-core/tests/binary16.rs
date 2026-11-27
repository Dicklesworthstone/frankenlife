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

    // Exhaustively check `F16::to_f32_4` across all 65,536 bit patterns (16,384 quads)
    // including exact NaN payload & sign preservation.
    for base in (0_u32..=u32::from(u16::MAX)).step_by(4) {
        let quad = [
            F16::from_bits(base as u16),
            F16::from_bits((base + 1) as u16),
            F16::from_bits((base + 2) as u16),
            F16::from_bits((base + 3) as u16),
        ];
        let got = F16::to_f32_4(quad);
        for (lane, (&h, &g)) in quad.iter().zip(&got).enumerate() {
            let bits = h.to_bits();
            let expected_bits = match (bits & 0x7C00) >> 10 {
                0 => {
                    let sign = u32::from(bits & 0x8000) << 16;
                    let frac = bits & 0x03FF;
                    sign | (f32::from(frac) * f32::from_bits(0x3380_0000)).to_bits()
                }
                0x1F => {
                    let sign = u32::from(bits & 0x8000) << 16;
                    sign | 0x7F80_0000 | (u32::from(bits & 0x03FF) << 13)
                }
                exp => {
                    let sign = u32::from(bits & 0x8000) << 16;
                    sign | ((u32::from(exp) + 112) << 23) | (u32::from(bits & 0x03FF) << 13)
                }
            };
            assert_eq!(
                g.to_bits(),
                expected_bits,
                "to_f32_4 lane={lane} bits={bits:#06x}"
            );
            assert_eq!(
                h.to_f32().to_bits(),
                expected_bits,
                "to_f32 bits={bits:#06x}"
            );
        }
    }
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

#[test]
fn to_f32_4x4_matches_independent_decode_for_all_patterns_and_mixed_tiles() {
    let independent_bits = |bits: u16| -> u32 {
        let sign = u32::from(bits & 0x8000) << 16;
        match (bits & 0x7C00) >> 10 {
            0 => sign | (f32::from(bits & 0x03FF) * f32::from_bits(0x3380_0000)).to_bits(),
            0x1F => sign | 0x7F80_0000 | (u32::from(bits & 0x03FF) << 13),
            exp => sign | ((u32::from(exp) + 112) << 23) | (u32::from(bits & 0x03FF) << 13),
        }
    };

    // Sweep every 16-pattern block across all 65,536 binary16 bit patterns
    // (4,096 tiles), covering both fast normal tiles and slow boundary tiles.
    let mut normal_tiles = 0_u32;
    let mut special_tiles = 0_u32;
    for base in (0_u32..=u32::from(u16::MAX)).step_by(16) {
        let mut tile = [[F16::ZERO; 4]; 4];
        let mut expect_all_normal = true;
        for (r, row) in tile.iter_mut().enumerate() {
            for (c, slot) in row.iter_mut().enumerate() {
                let bits = (base + (r * 4 + c) as u32) as u16;
                *slot = F16::from_bits(bits);
                let exp = (bits >> 10) & 0x1F;
                if exp == 0 || exp == 0x1F {
                    expect_all_normal = false;
                }
            }
        }
        assert_eq!(F16::all_normal_4x4(tile), expect_all_normal);
        if expect_all_normal {
            normal_tiles += 1;
        } else {
            special_tiles += 1;
        }
        let got = F16::to_f32_4x4(tile);
        for (r, (t_row, g_row)) in tile.iter().zip(&got).enumerate() {
            for (c, (&h, &g)) in t_row.iter().zip(g_row).enumerate() {
                let bits = h.to_bits();
                assert_eq!(
                    g.to_bits(),
                    independent_bits(bits),
                    "tile base={base:#06x} ({r},{c}) bits={bits:#06x}"
                );
            }
        }
    }
    assert!(normal_tiles > 3_500);
    assert!(special_tiles > 200);

    // Inject each tricky special value (+0, -0, min/max subnormal, ±Inf,
    // signaling/quiet NaN with both signs, smallest/largest normals) into every
    // one of the 16 tile positions amidst 15 normal values.
    let specials = [
        0x0000_u16, // +0.0
        0x8000,     // -0.0
        0x0001,     // smallest +subnormal
        0x8001,     // smallest -subnormal
        0x03FF,     // largest +subnormal
        0x83FF,     // largest -subnormal
        0x0400,     // smallest +normal (fast path!)
        0x8400,     // smallest -normal (fast path!)
        0x7BFF,     // largest +normal (fast path!)
        0xFBFF,     // largest -normal (fast path!)
        0x7C00,     // +Inf
        0xFC00,     // -Inf
        0x7C01,     // +sNaN payload 1
        0xFC01,     // -sNaN payload 1
        0x7E00,     // +qNaN
        0xFFFF,     // -qNaN all ones
    ];
    for &special in &specials {
        let special_is_normal = {
            let exp = (special >> 10) & 0x1F;
            exp != 0 && exp != 0x1F
        };
        for pos in 0..16 {
            let mut tile = [[F16::from_bits(0x3C00); 4]; 4];
            tile[pos / 4][pos % 4] = F16::from_bits(special);
            assert_eq!(
                F16::all_normal_4x4(tile),
                special_is_normal,
                "special={special:#06x} pos={pos}"
            );
            let got = F16::to_f32_4x4(tile);
            for (r, (t_row, g_row)) in tile.iter().zip(&got).enumerate() {
                for (c, (&h, &g)) in t_row.iter().zip(g_row).enumerate() {
                    assert_eq!(
                        g.to_bits(),
                        independent_bits(h.to_bits()),
                        "special={special:#06x} pos={pos} at ({r},{c})"
                    );
                }
            }
        }
    }
}
