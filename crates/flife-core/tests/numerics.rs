//! Host-independent elementary functions (plan A.1, C.11): pinned bits,
//! distance from correctly rounded values, agreement with the platform
//! `libm`, and the domain and symmetry each function promises.

use flife_core::numerics::{exp, exp_neg, expm1_neg, ln, log2, sigmoid, sin_cos_turns, tanh};

/// A total order on finite `f64` in which adjacent values differ by one.
fn ordinal(value: f64) -> i64 {
    let bits = value.to_bits();
    let magnitude = i64::try_from(bits & 0x7fff_ffff_ffff_ffff).unwrap_or(i64::MAX);
    if bits >> 63 == 0 {
        magnitude
    } else {
        -magnitude
    }
}

fn ulps(a: f64, b: f64) -> u64 {
    ordinal(a).abs_diff(ordinal(b))
}

/// Log-spaced from `1e-9` past the last argument, plus a dense band where
/// the reductions switch branches.
fn sweep(limit: f64) -> Vec<f64> {
    let mut points = Vec::new();
    let mut x = 1.0e-9_f64;
    while x < limit {
        points.push(x);
        x *= 1.031;
    }
    points.extend((0..400).map(|k| 0.02 + f64::from(k) * 0.005));
    points
}

type Function = fn(f64) -> Option<f64>;

/// `(function, name, x, our bits, correctly rounded bits)`. The correctly
/// rounded values come from Python `Decimal` at 60 digits. Our bits are the
/// cross-host guard: pinned on macOS arm64 and checked again on Linux
/// x86_64.
const PINS: [(Function, &str, f64, u64, u64); 24] = [
    (
        exp_neg,
        "exp_neg",
        0.01 / 128.0,
        0x3fef_ff5c_2a99_2df2,
        0x3fef_ff5c_2a99_2df2,
    ),
    (
        exp_neg,
        "exp_neg",
        0.1,
        0x3fec_f46d_99d5_2b3a,
        0x3fec_f46d_99d5_2b3a,
    ),
    (
        exp_neg,
        "exp_neg",
        1.0,
        0x3fd7_8b56_362c_ef38,
        0x3fd7_8b56_362c_ef38,
    ),
    (
        exp_neg,
        "exp_neg",
        8.0,
        0x3f35_fc21_0410_27ad,
        0x3f35_fc21_0410_27ad,
    ),
    (
        exp_neg,
        "exp_neg",
        10.0,
        0x3f07_cd79_b564_7c9a,
        0x3f07_cd79_b564_7c9b,
    ),
    (
        exp_neg,
        "exp_neg",
        30.0,
        0x3d3a_56e0_c2ac_7f75,
        0x3d3a_56e0_c2ac_7f75,
    ),
    (
        exp_neg,
        "exp_neg",
        700.0,
        0x00d1_4f2b_0fb9_307f,
        0x00d1_4f2b_0fb9_307f,
    ),
    (
        exp_neg,
        "exp_neg",
        745.1,
        0x0000_0000_0000_0001,
        0x0000_0000_0000_0001,
    ),
    (
        expm1_neg,
        "expm1_neg",
        1.0e-6 / 128.0,
        0x3e40_c6f7_9f9c_73f5,
        0x3e40_c6f7_9f9c_73f5,
    ),
    (
        expm1_neg,
        "expm1_neg",
        0.1,
        0x3fb8_5c93_3156_a62c,
        0x3fb8_5c93_3156_a62c,
    ),
    (
        expm1_neg,
        "expm1_neg",
        0.5,
        0x3fd9_2e9a_0720_d3ec,
        0x3fd9_2e9a_0720_d3ec,
    ),
    (
        expm1_neg,
        "expm1_neg",
        1.0,
        0x3fe4_3a54_e4e9_8864,
        0x3fe4_3a54_e4e9_8864,
    ),
    (
        expm1_neg,
        "expm1_neg",
        10.0,
        0x3fef_ffa0_ca19_2a6e,
        0x3fef_ffa0_ca19_2a6e,
    ),
    (
        tanh,
        "tanh",
        1.0e-6 / 128.0,
        0x3e40_c6f7_a0b5_ed8d,
        0x3e40_c6f7_a0b5_ed8d,
    ),
    (
        tanh,
        "tanh",
        0.1,
        0x3fb9_83d7_795f_4139,
        0x3fb9_83d7_795f_413a,
    ),
    (
        tanh,
        "tanh",
        0.5,
        0x3fdd_9353_d756_8af3,
        0x3fdd_9353_d756_8af3,
    ),
    (
        tanh,
        "tanh",
        4.15,
        0x3fef_fbed_e763_2268,
        0x3fef_fbed_e763_2268,
    ),
    (
        tanh,
        "tanh",
        19.0,
        0x3fef_ffff_ffff_ffff,
        0x3fef_ffff_ffff_ffff,
    ),
    (
        sigmoid,
        "sigmoid",
        0.1,
        0x3fe0_cca1_2729_afb8,
        0x3fe0_cca1_2729_afb8,
    ),
    (
        sigmoid,
        "sigmoid",
        -0.5,
        0x3fd8_29a0_5659_78df,
        0x3fd8_29a0_5659_78df,
    ),
    (
        sigmoid,
        "sigmoid",
        8.0,
        0x3fef_fd40_b845_05a2,
        0x3fef_fd40_b845_05a1,
    ),
    (
        sigmoid,
        "sigmoid",
        -8.0,
        0x3f35_fa3d_d7d2_f7a7,
        0x3f35_fa3d_d7d2_f7a6,
    ),
    (
        sigmoid,
        "sigmoid",
        -10.0,
        0x3f07_cd32_e41d_d95f,
        0x3f07_cd32_e41d_d960,
    ),
    (
        sigmoid,
        "sigmoid",
        30.0,
        0x3fef_ffff_ffff_fcb6,
        0x3fef_ffff_ffff_fcb5,
    ),
];

#[test]
fn pinned_bits_are_within_one_ulp_of_correctly_rounded() {
    for (function, name, x, ours, correct) in PINS {
        let value = function(x).unwrap_or(f64::NAN);
        assert_eq!(value.to_bits(), ours, "{name}({x:?}) = {value:e}");
        let error = ulps(value, f64::from_bits(correct));
        assert!(
            error <= 1,
            "{name}({x:?}) is {error} ulp from correctly rounded"
        );
    }
}

#[test]
fn every_function_tracks_the_platform_libm() {
    // Measured against correctly rounded values over this sweep (7,257
    // evaluations): `exp_neg` ≤ 1 ulp; `expm1_neg`, `tanh` and `sigmoid`
    // ≤ 2 ulp. A conforming `libm` is within 1 ulp, and the `std` logistic
    // within 2, so these bounds hold on every host without pinning `libm`.
    for x in sweep(745.0) {
        let ours = exp_neg(x).unwrap_or(f64::NAN);
        assert!(ulps(ours, (-x).exp()) <= 2, "exp_neg({x:e})");
        if x < 41.0 {
            let ours = expm1_neg(x).unwrap_or(f64::NAN);
            assert!(ulps(ours, -(-x).exp_m1()) <= 3, "expm1_neg({x:e})");
            for t in [x, -x] {
                let ours = sigmoid(t).unwrap_or(f64::NAN);
                assert!(ulps(ours, 1.0 / (1.0 + (-t).exp())) <= 4, "sigmoid({t:e})");
            }
        }
        if x < 21.0 {
            for t in [x, -x] {
                let ours = tanh(t).unwrap_or(f64::NAN);
                assert!(ulps(ours, t.tanh()) <= 3, "tanh({t:e})");
            }
        }
    }
}

#[test]
fn domains_saturation_and_symmetry() {
    for x in [-1.0e-300, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(exp_neg(x), None, "exp_neg({x})");
        assert_eq!(expm1_neg(x), None, "expm1_neg({x})");
    }
    for x in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(tanh(x), None, "tanh({x})");
        assert_eq!(sigmoid(x), None, "sigmoid({x})");
    }
    assert_eq!(exp_neg(0.0), Some(1.0));
    assert_eq!(exp_neg(746.0), Some(0.0));
    assert_eq!(exp_neg(1.0e300), Some(0.0));
    assert_eq!(expm1_neg(0.0), Some(0.0));
    assert_eq!(expm1_neg(40.0), Some(1.0));
    assert_eq!(tanh(0.0).map(f64::to_bits), Some(0.0_f64.to_bits()));
    assert_eq!(tanh(-0.0).map(f64::to_bits), Some((-0.0_f64).to_bits()));
    assert_eq!(tanh(20.0), Some(1.0));
    assert_eq!(tanh(-1.0e300), Some(-1.0));
    assert_eq!(sigmoid(0.0), Some(0.5));
    // Large negative logits decay smoothly instead of dividing by infinity.
    assert_eq!(sigmoid(-1.0e300), Some(0.0));
    assert!(sigmoid(-700.0).is_some_and(|v| v > 0.0));
    assert_eq!(sigmoid(1.0e300), Some(1.0));

    for x in sweep(745.0) {
        let e = exp_neg(x).unwrap_or(f64::NAN);
        let m = expm1_neg(x).unwrap_or(f64::NAN);
        let t = tanh(x).unwrap_or(f64::NAN);
        let s = sigmoid(x).unwrap_or(f64::NAN);
        assert_eq!(
            tanh(-x).map(f64::to_bits),
            Some((-t).to_bits()),
            "odd at {x:e}"
        );
        let mirrored = sigmoid(-x).unwrap_or(f64::NAN);
        assert!(
            (s + mirrored - 1.0).abs() <= f64::EPSILON,
            "σ(x) + σ(−x) at {x:e}"
        );
        assert!((0.0..=1.0).contains(&e) && (0.0..=1.0).contains(&m));
        assert!((0.0..=1.0).contains(&t) && (0.5..=1.0).contains(&s));
    }
    let mut sorted = sweep(745.0);
    sorted.sort_by(f64::total_cmp);
    for pair in sorted.windows(2) {
        let [a, b] = [pair[0], pair[1]];
        for (name, f) in [("exp_neg", exp_neg as Function), ("expm1_neg", expm1_neg)] {
            let (fa, fb) = (f(a).unwrap_or(f64::NAN), f(b).unwrap_or(f64::NAN));
            let monotone = if name == "exp_neg" {
                fb <= fa
            } else {
                fb >= fa
            };
            assert!(monotone, "{name} not monotone between {a:e} and {b:e}");
        }
        if b < 21.0 {
            let (ta, tb) = (tanh(a).unwrap_or(f64::NAN), tanh(b).unwrap_or(f64::NAN));
            assert!(tb >= ta, "tanh not monotone between {a:e} and {b:e}");
        }
        let (sa, sb) = (
            sigmoid(a).unwrap_or(f64::NAN),
            sigmoid(b).unwrap_or(f64::NAN),
        );
        assert!(sb >= sa, "sigmoid not monotone between {a:e} and {b:e}");
    }
}

#[test]
fn expm1_neg_resolves_drag_where_naive_forms_cancel() {
    // Plan C.11: γ = 1e-6 s⁻¹ and Δt = 1/128 s. The drag-integrated force
    // coefficient (1 − e^(−γΔt))/γ is Δt to first order.
    let gamma = 1.0e-6_f64;
    let step = 1.0 / 128.0;
    let coefficient = expm1_neg(gamma * step).unwrap_or(f64::NAN) / gamma;
    assert!(
        (coefficient - 0.007_812_499_969_48).abs() < 1.0e-14,
        "{coefficient}"
    );
    // In F32 the naive form loses everything: e^(−7.8e-9) rounds to 1.
    let naive = 1.0_f32 - (-(gamma as f32) * (step as f32)).exp();
    assert_eq!(naive, 0.0);
}

#[test]
fn sigmoid_of_tanh_is_not_sigmoid() {
    // Plan C.11: an activation applied twice is a different function.
    let once = sigmoid(8.0).unwrap_or(f64::NAN);
    let twice = tanh(8.0).and_then(sigmoid).unwrap_or(f64::NAN);
    assert!((once - 0.999_664_65).abs() < 5.0e-9, "{once}");
    assert!((twice - 0.731_058_53).abs() < 1.0e-8, "{twice}");
}

#[test]
fn log2_is_exact_at_powers_of_two_and_close_elsewhere() {
    // Pyramid levels sit at powers of two: the blend weight there must be
    // exactly zero (§6.1), including across the subnormal range.
    for exponent in -1074..=1023_i32 {
        let x = if exponent >= -1022 {
            f64::from_bits(u64::try_from(exponent + 1023).unwrap_or(0) << 52)
        } else {
            f64::from_bits(1_u64 << (exponent + 1074))
        };
        assert_eq!(log2(x), Some(f64::from(exponent)), "2^{exponent}");
    }
    // `(x, our bits, correctly rounded bits)` from Python `Decimal`.
    for (x, ours, correct) in [
        (3.0, 0x3ff9_5c01_a39f_bd69_u64, 0x3ff9_5c01_a39f_bd68_u64),
        (1.5, 0x3fe2_b803_473f_7ad2, 0x3fe2_b803_473f_7ad1),
        (1.42, 0x3fe0_3042_2cd1_0ffb, 0x3fe0_3042_2cd1_0ffb),
        (10.0, 0x400a_934f_0979_a371, 0x400a_934f_0979_a371),
        (1.0e-300, 0xc08f_24a0_9f1a_8b89, 0xc08f_24a0_9f1a_8b89),
        (
            1.0 + f64::EPSILON,
            0x3cb7_1547_652b_82fe,
            0x3cb7_1547_652b_82fd,
        ),
        (f64::MAX, 0x4090_0000_0000_0000, 0x4090_0000_0000_0000),
    ] {
        let value = log2(x).unwrap_or(f64::NAN);
        assert_eq!(value.to_bits(), ours, "log2({x:e}) = {value:e}");
        assert!(ulps(value, f64::from_bits(correct)) <= 1, "log2({x:e})");
    }
    // Measured: within 2 ulp of correctly rounded over 4,078 arguments from
    // 1e-12 to 1e12. A conforming `log2` is within 1.
    let mut x = 1.0e-12_f64;
    while x < 1.0e12 {
        let ours = log2(x).unwrap_or(f64::NAN);
        assert!(ulps(ours, x.log2()) <= 3, "log2({x:e})");
        x *= 1.0137;
    }
    for x in [0.0, -0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(log2(x), None, "log2({x})");
    }
}

/// `(x, our bits, correctly rounded bits)`; the correctly rounded values come
/// from Python `Decimal` at 80 digits. Our bits are the cross-host guard.
const LN_PINS: [(f64, u64, u64); 11] = [
    (0.5, 0xbfe6_2e42_fefa_39ef, 0xbfe6_2e42_fefa_39ef),
    // The smallest and largest open uniforms Box–Muller takes.
    (
        1.0 / 9_007_199_254_740_992.0,
        0xc042_5e4f_7b27_37fa,
        0xc042_5e4f_7b27_37fa,
    ),
    (
        1.0 - 1.0 / 9_007_199_254_740_992.0,
        0xbca0_0000_0000_0000,
        0xbca0_0000_0000_0000,
    ),
    (1.0e-300, 0xc085_9634_47f8_7fb5, 0xc085_9634_47f8_7fb5),
    (0.1, 0xc002_6bb1_bbb5_5515, 0xc002_6bb1_bbb5_5515),
    (0.75, 0xbfd2_6962_1134_db92, 0xbfd2_6962_1134_db92),
    (3.0, 0x3ff1_93ea_7aad_030b, 0x3ff1_93ea_7aad_030b),
    (16.0, 0x4006_2e42_fefa_39ef, 0x4006_2e42_fefa_39ef),
    (5.0e-324, 0xc087_4385_446d_71c3, 0xc087_4385_446d_71c3),
    (
        1.0 + f64::EPSILON,
        0x3cb0_0000_0000_0000,
        0x3caf_ffff_ffff_ffff,
    ),
    (
        core::f64::consts::FRAC_1_SQRT_2,
        0xbfd6_2e42_fefa_39ed,
        0xbfd6_2e42_fefa_39ee,
    ),
];

/// As [`LN_PINS`], for `exp`.
const EXP_PINS: [(f64, u64, u64); 8] = [
    (0.0625, 0x3ff1_082b_577d_34ee, 0x3ff1_082b_577d_34ed),
    (0.5, 0x3ffa_6129_8e1e_069c, 0x3ffa_6129_8e1e_069c),
    (1.0, 0x4005_bf0a_8b14_576a, 0x4005_bf0a_8b14_5769),
    // `ln 16` rounded down, so `e^x` is just below 16.
    (
        2.772_588_722_239_781,
        0x402f_ffff_ffff_ffff,
        0x402f_ffff_ffff_ffff,
    ),
    (100.0, 0x48f3_494a_9b17_1bf5, 0x48f3_494a_9b17_1bf5),
    (709.0, 0x7fdd_422d_2be5_dc9b, 0x7fdd_422d_2be5_dc9b),
    (1.0e-10, 0x3ff0_0000_0006_df38, 0x3ff0_0000_0006_df38),
    (
        0.346_573_590_279_972_64,
        0x3ff6_a09e_667f_3bcc,
        0x3ff6_a09e_667f_3bcc,
    ),
];

/// `(u, our sin 2πu, our cos 2πu, correctly rounded sin, correctly rounded
/// cos)`.
const TURN_PINS: [(f64, u64, u64, u64, u64); 9] = [
    (
        0.1,
        0x3fe2_cf23_0475_5a5e,
        0x3fe9_e377_9b97_f4a8,
        0x3fe2_cf23_0475_5a5e,
        0x3fe9_e377_9b97_f4a8,
    ),
    (
        0.125,
        0x3fe6_a09e_667f_3bcc,
        0x3fe6_a09e_667f_3bcd,
        0x3fe6_a09e_667f_3bcd,
        0x3fe6_a09e_667f_3bcd,
    ),
    (
        0.2,
        0x3fee_6f0e_1344_5500,
        0x3fd3_c6ef_372f_e94e,
        0x3fee_6f0e_1344_5500,
        0x3fd3_c6ef_372f_e94e,
    ),
    (
        0.3,
        0x3fee_6f0e_1344_5500,
        0xbfd3_c6ef_372f_e94e,
        0x3fee_6f0e_1344_5500,
        0xbfd3_c6ef_372f_e94e,
    ),
    (
        0.6,
        0xbfe2_cf23_0475_5a5d,
        0xbfe9_e377_9b97_f4a8,
        0xbfe2_cf23_0475_5a5d,
        0xbfe9_e377_9b97_f4a8,
    ),
    (
        0.7,
        0xbfee_6f0e_1344_54ff,
        0xbfd3_c6ef_372f_e954,
        0xbfee_6f0e_1344_54ff,
        0xbfd3_c6ef_372f_e954,
    ),
    (
        0.9,
        0xbfe2_cf23_0475_5a5d,
        0x3fe9_e377_9b97_f4a8,
        0xbfe2_cf23_0475_5a5d,
        0x3fe9_e377_9b97_f4a8,
    ),
    // The first and last half-open uniforms.
    (
        f64::EPSILON,
        0x3cd9_21fb_5444_2d18,
        0x3ff0_0000_0000_0000,
        0x3cd9_21fb_5444_2d18,
        0x3ff0_0000_0000_0000,
    ),
    (
        1.0 - f64::EPSILON,
        0xbcd9_21fb_5444_2d18,
        0x3ff0_0000_0000_0000,
        0xbcd9_21fb_5444_2d18,
        0x3ff0_0000_0000_0000,
    ),
];

#[test]
fn ln_and_exp_are_within_two_ulp_of_correctly_rounded() {
    for (x, ours, correct) in LN_PINS {
        let value = ln(x).unwrap_or(f64::NAN);
        assert_eq!(value.to_bits(), ours, "ln({x:e}) = {value:e}");
        assert!(ulps(value, f64::from_bits(correct)) <= 2, "ln({x:e})");
    }
    for (x, ours, correct) in EXP_PINS {
        let value = exp(x).unwrap_or(f64::NAN);
        assert_eq!(value.to_bits(), ours, "exp({x:e}) = {value:e}");
        assert!(ulps(value, f64::from_bits(correct)) <= 1, "exp({x:e})");
    }
    // Measured against correctly rounded values (scratch sweep, 12,387 `ln`
    // and 3,292 `exp` arguments): `ln` ≤ 2 ulp, `exp` ≤ 1 ulp. A conforming
    // `libm` is within 1, so these bounds hold on every host.
    let mut x = 1.0e-300_f64;
    while x < 1.0e300 {
        assert!(ulps(ln(x).unwrap_or(f64::NAN), x.ln()) <= 3, "ln({x:e})");
        x *= 1.37;
    }
    for t in sweep(709.0) {
        for t in [t, -t] {
            assert!(ulps(exp(t).unwrap_or(f64::NAN), t.exp()) <= 2, "exp({t:e})");
        }
    }
    // Exact where the functions promise it.
    assert_eq!(ln(1.0), Some(0.0));
    assert_eq!(exp(0.0), Some(1.0));
    for x in [1.0e-9, 0.01, 0.5, 3.0, 40.0, 745.0] {
        assert_eq!(exp(-x), exp_neg(x), "exp(-{x}) is exp_neg({x})");
    }
    for x in [0.0, -0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(ln(x), None, "ln({x})");
    }
    for x in [710.0, 1.0e300, f64::NAN, f64::INFINITY] {
        assert_eq!(exp(x), None, "exp({x})");
    }
    assert_eq!(exp(f64::NEG_INFINITY), None);
    // `log2` and `ln` share their reduction.
    for x in [0.3, 7.0, 1.0e-200] {
        let ratio = ln(x).unwrap_or(f64::NAN) / log2(x).unwrap_or(f64::NAN);
        assert!((ratio - core::f64::consts::LN_2).abs() < 4.0e-16, "{x}");
    }
}

#[test]
fn turn_angles_are_within_two_ulp_and_exact_at_quarter_turns() {
    for (u, our_sin, our_cos, sin, cos) in TURN_PINS {
        let (s, c) = sin_cos_turns(u).unwrap_or((f64::NAN, f64::NAN));
        assert_eq!(
            (s.to_bits(), c.to_bits()),
            (our_sin, our_cos),
            "turns({u:e})"
        );
        assert!(ulps(s, f64::from_bits(sin)) <= 2, "sin 2π·{u:e}");
        assert!(ulps(c, f64::from_bits(cos)) <= 2, "cos 2π·{u:e}");
    }
    // Quarter turns are exact; the sign of a zero follows the quadrant.
    for (u, expected) in [
        (0.0, (0.0, 1.0)),
        (0.25, (1.0, 0.0)),
        (0.5, (0.0, -1.0)),
        (0.75, (-1.0, 0.0)),
    ] {
        assert_eq!(sin_cos_turns(u), Some(expected), "turns({u})");
    }
    // Measured: within 2 ulp of correctly rounded over 4,096 turns (scratch
    // sweep). Against `libm` the angle `2πu` itself rounds, so compare
    // absolutely; a unit vector stays a unit vector to rounding.
    for k in 0..4096_u32 {
        let u = (f64::from(k) + 0.37) / 4096.0;
        let (s, c) = sin_cos_turns(u).unwrap_or((f64::NAN, f64::NAN));
        let angle = core::f64::consts::TAU * u;
        assert!((s - angle.sin()).abs() < 2.0e-15, "sin 2π·{u}");
        assert!((c - angle.cos()).abs() < 2.0e-15, "cos 2π·{u}");
        assert!((s * s + c * c - 1.0).abs() < 4.0 * f64::EPSILON, "{u}");
    }
    for u in [1.0, -1.0e-300, 2.0, f64::NAN, f64::INFINITY] {
        assert_eq!(sin_cos_turns(u), None, "turns({u})");
    }
}
