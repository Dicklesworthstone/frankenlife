//! Host-independent elementary functions (plan A.1, C.11): pinned bits,
//! distance from correctly rounded values, agreement with the platform
//! `libm`, and the domain and symmetry each function promises.

use flife_core::numerics::{
    exp, exp_neg, exp_neg_4, expm1_neg, expm1_neg_4, ln, log2, normalize_turns, pow_positive,
    shrink_to_left_fold_norm_limit, sigmoid, sigmoid_4, sin_cos_turns, symplectic_rotate_channels,
    tanh, tanh_4, tanh_f32, tanh_f32_4, tanh_f64_as_f32, tanh_f64_as_f32_4,
};

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

/// How one F32 argument of `tanh_f32` is reached, by its magnitude.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Zero,
    Series,
    Exponential,
    Saturated,
    Nonfinite,
}

fn route(z: f32) -> Route {
    let a = z.abs();
    if !a.is_finite() {
        Route::Nonfinite
    } else if a == 0.0 {
        Route::Zero
    } else if 2.0 * f64::from(a) < 1.0 / 16.0 {
        Route::Series
    } else if a < 20.0 {
        Route::Exponential
    } else {
        Route::Saturated
    }
}

/// `tanh_f32` is `tanh` in F64 rounded once to F32 for every one of the
/// 2^32 F32 bit patterns: both signs, both zeros, the subnormals, the
/// saturated range, the infinities and every NaN. The patterns are split
/// across all cores.
#[test]
fn tanh_f32_is_the_reference_tanh_rounded_to_f32_for_every_f32() {
    let threads = std::thread::available_parallelism().map_or(8, usize::from);
    let patterns = 1_u64 << 32;
    let length = patterns.div_ceil(threads as u64);
    let results: Vec<([u64; 5], u64, Option<u32>)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads as u64)
            .map(|worker| {
                scope.spawn(move || {
                    let mut routes = [0_u64; 5];
                    let mut wrong = 0_u64;
                    let mut first = None;
                    for bits in (worker * length)..((worker + 1) * length).min(patterns) {
                        let bits = u32::try_from(bits).unwrap();
                        let z = f32::from_bits(bits);
                        routes[route(z) as usize] += 1;
                        let want = tanh(f64::from(z)).map(|t| (t as f32).to_bits());
                        if tanh_f32(z).map(f32::to_bits) != want {
                            wrong += 1;
                            first.get_or_insert(bits);
                        }
                    }
                    (routes, wrong, first)
                })
            })
            .collect();
        workers.into_iter().map(|w| w.join().unwrap()).collect()
    });
    let mut routes = [0_u64; 5];
    for (counted, _, _) in &results {
        for (total, count) in routes.iter_mut().zip(counted) {
            *total += count;
        }
    }
    let wrong: u64 = results.iter().map(|(_, wrong, _)| wrong).sum();
    let first = results.iter().find_map(|(_, _, first)| *first);
    assert_eq!(
        routes.iter().sum::<u64>(),
        patterns,
        "every pattern checked"
    );
    // Per sign: one zero; the series below |z| = 2^-5 (bits 0x3d00_0000);
    // the exponential from there to 20 (bits 0x41a0_0000); the saturated
    // range up to the infinity (bits 0x7f80_0000); 2^23 - 1 NaN payloads
    // and the infinity.
    assert_eq!(
        routes,
        [
            2,
            2 * (0x3d00_0000 - 1),
            2 * (0x41a0_0000 - 0x3d00_0000),
            2 * (0x7f80_0000 - 0x41a0_0000),
            1 << 24,
        ],
        "patterns by route: zero, series, exponential, saturated, nonfinite"
    );
    assert_eq!(
        wrong,
        0,
        "{wrong} patterns differ; the first is {:?} ({first:#x?})",
        first.map(f32::from_bits)
    );
}

/// `tanh_f64_as_f32(x)` matches `tanh(x).map(|t| t as f32)` bit for bit across
/// edges, branch boundaries, a wide sweep of F64 values, and adversarial F64
/// inputs placed right on F32 rounding midpoints (`atanh(k + 0.5 ulp_f32)`).
#[test]
fn tanh_f64_as_f32_matches_tanh_rounded_including_f32_midpoints() {
    let check = |x: f64| {
        let want = tanh(x).map(|t| (t as f32).to_bits());
        let got = tanh_f64_as_f32(x).map(f32::to_bits);
        assert_eq!(
            got,
            want,
            "tanh_f64_as_f32({x:?}) ({:#x}) gave {got:#x?}, want {want:#x?}",
            x.to_bits()
        );
    };

    for x in [
        0.0,
        -0.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MIN_POSITIVE,
        -f64::MIN_POSITIVE,
        f64::from_bits(1),
        f64::from(f32::MIN_POSITIVE),
        f64::from(f32::MIN_POSITIVE) * 0.5,
        f64::from(f32::MIN_POSITIVE) * 2.0,
        1.0 / 32.0,
        (1.0_f64 / 32.0).next_down(),
        (1.0_f64 / 32.0).next_up(),
        1.0,
        1.0_f64.next_down(),
        1.0_f64.next_up(),
        20.0,
        20.0_f64.next_down(),
        -20.0,
        100.0,
        -100.0,
    ] {
        check(x);
    }

    // Wide deterministic sweep across (-21, 21), spanning 2^-150 .. 21.
    let mut state = 0x5EED_CAFE_BABE_1234_u64;
    for _ in 0..200_000 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let sign = if state & 1 == 0 { 1.0 } else { -1.0 };
        let exp_idx = ((state >> 1) % 160) as i32 - 155; // 2^-155 .. 2^4
        let frac = ((state >> 11) as f64) / ((1_u64 << 53) as f64);
        let x = sign * (1.0 + frac) * 2.0_f64.powi(exp_idx);
        check(x);
    }

    // Adversarial F32 midpoints: y = f32 + 0.5 ulp_f32, x = atanh(y), plus
    // nearby F64 neighbors around each tie.
    for exp in -30_i32..0 {
        let base = 2.0_f64.powi(exp);
        let half_ulp_f32 = 2.0_f64.powi(exp - 24);
        for k in 0..200_u32 {
            let y_mid = base + (f64::from(k) * 2.0 + 1.0) * half_ulp_f32;
            if y_mid >= 1.0 {
                break;
            }
            let x_mid = y_mid.atanh();
            let b = x_mid.to_bits();
            for delta in [-16_i64, -4, -1, 0, 1, 4, 16] {
                let x = f64::from_bits(b.wrapping_add_signed(delta));
                check(x);
                check(-x);
            }
        }
    }
}

#[test]
fn pow_positive_is_host_independent_exact_on_identities_and_tracks_libm() {
    // Domain refusals and overflow.
    for (base, exponent) in [
        (0.0, 1.0),
        (-1.0, 2.0),
        (f64::NAN, 1.0),
        (f64::INFINITY, 1.0),
        (2.0, f64::NAN),
        (2.0, f64::INFINITY),
        (2.0, f64::NEG_INFINITY),
        (1.0e200, 4.0),
    ] {
        assert_eq!(
            pow_positive(base, exponent),
            None,
            "pow_positive({base:?}, {exponent:?}) must refuse"
        );
    }
    // Exact identities: x^0 = 1, x^1 = x (preserving exact bits), 1^a = 1.
    for x in [f64::MIN_POSITIVE, 0.125, 0.3, 1.0, 2.75, f64::MAX] {
        assert_eq!(
            pow_positive(x, 0.0).map(f64::to_bits),
            Some(1.0_f64.to_bits())
        );
        assert_eq!(
            pow_positive(x, -0.0).map(f64::to_bits),
            Some(1.0_f64.to_bits())
        );
        assert_eq!(pow_positive(x, 1.0).map(f64::to_bits), Some(x.to_bits()));
    }
    for a in [-10.0, -0.5, 0.0, 0.5, 1.5, 2.0, 100.0] {
        assert_eq!(
            pow_positive(1.0, a).map(f64::to_bits),
            Some(1.0_f64.to_bits())
        );
    }
    // Agreement with platform libm across sensing-scale ratios and exponents.
    for base in [0.0625, 0.25, 0.5, 0.75, 1.25, 2.0, 3.0, 4.0, 8.0, 16.0] {
        for exponent in [0.25, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0] {
            let got = pow_positive(base, exponent).expect("finite power");
            let want = base.powf(exponent);
            let rel = (got - want).abs() / want;
            assert!(
                rel <= 1.0e-14,
                "pow_positive({base}, {exponent}) = {got:e} vs {want:e} (rel {rel:e})"
            );
        }
    }
    // Cross-host pinned bits (exp(exponent * ln(base))).
    assert_eq!(
        pow_positive(2.0, 1.5).expect("2^1.5").to_bits(),
        exp(1.5 * ln(2.0).expect("ln")).expect("exp").to_bits()
    );
    assert_eq!(
        pow_positive(3.0, 2.0).expect("3^2").to_bits(),
        exp(2.0 * ln(3.0).expect("ln")).expect("exp").to_bits()
    );
}

#[test]
fn numerics_normalize_turns_shrink_and_symplectic_channels() {
    // 1. normalize_turns handles negative, boundary, and IEEE-754 -1e-17 traps.
    assert_eq!(normalize_turns(-1.0e-17), Some(0.0));
    assert_eq!(normalize_turns(-0.25), Some(0.75));
    assert_eq!(normalize_turns(1.0), Some(0.0));
    assert_eq!(normalize_turns(-0.0), Some(0.0));
    assert_eq!(normalize_turns(2.25), Some(0.25));
    assert_eq!(normalize_turns(-3.75), Some(0.25));
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(normalize_turns(bad), None);
    }
    for u in [-1.0e-17, -0.25, -0.5, 0.0, 0.5, 1.0, 3.125, -7.875] {
        let wrapped = normalize_turns(u).expect("finite angle normalizes");
        assert!((0.0..1.0).contains(&wrapped), "{u} -> {wrapped}");
        assert!(sin_cos_turns(wrapped).is_some(), "sin_cos_turns({wrapped})");
    }

    // 2. shrink_to_left_fold_norm_limit fixes adversarial 32-D left-fold 1.0 + 2 ulp.
    let left_fold_sq = |values: &[f64; 32]| values.iter().fold(0.0_f64, |t, &c| t + c * c);
    let base = (1.0_f64 / 32.0).sqrt();
    let mut adv = [base; 32];
    while left_fold_sq(&adv) <= 1.0 {
        adv[0] = adv[0].next_up();
    }
    // Bump one extra ulp so left-fold is strictly 1.0 + >=2 ulp.
    adv[1] = adv[1].next_up();
    let before = left_fold_sq(&adv);
    assert!(
        before > 1.0,
        "adversarial setup must exceed 1.0: {before:?}"
    );
    shrink_to_left_fold_norm_limit(&mut adv, 1.0).expect("shrink brings inside unit ball");
    let after = left_fold_sq(&adv);
    assert!(after <= 1.0, "left-fold squared norm {after:?} > 1.0");
    assert!(after > 1.0 - 1.0e-12, "shrink should be minimal: {after:?}");

    // Already-valid vectors stay bit-identical.
    let snapshot = adv.map(f64::to_bits);
    shrink_to_left_fold_norm_limit(&mut adv, 1.0).expect("idempotent");
    assert_eq!(adv.map(f64::to_bits), snapshot);

    // Invalid inputs are refused.
    assert!(shrink_to_left_fold_norm_limit(&mut adv, 0.0).is_err());
    assert!(shrink_to_left_fold_norm_limit(&mut adv, -1.0).is_err());
    assert!(shrink_to_left_fold_norm_limit(&mut adv, f64::NAN).is_err());
    let mut nan_v = [0.0; 32];
    nan_v[7] = f64::NAN;
    assert!(shrink_to_left_fold_norm_limit(&mut nan_v, 1.0).is_err());

    // 3. symplectic_rotate_channels on active_channels = 32 and 24.
    let mut a = [0.0_f64; 32];
    for (idx, slot) in a.iter_mut().enumerate() {
        *slot = ((idx as f64) * 0.37 - 5.2).sin();
    }
    shrink_to_left_fold_norm_limit(&mut a, 1.0).expect("normalize a");

    for active in [32_usize, 24_usize] {
        let ja = symplectic_rotate_channels(&a, active, 1.0).expect("symplectic rotate");
        assert!(
            left_fold_sq(&ja) <= 1.0,
            "active={active} left-fold norm exceeded 1.0"
        );
        let dot: f64 = a.iter().zip(ja.iter()).map(|(x, y)| x * y).sum();
        assert!(
            dot.abs() < 1.0e-15,
            "active={active}: a · J a = {dot:e} is not orthogonal"
        );
        for &tail in &ja[active..] {
            assert_eq!(tail, 0.0, "active={active} tail channels must be zero");
        }
        // Check exact 2D symplectic pair structure (up to uniform shrink factor <= 1).
        for pair in 0..(active / 2) {
            let even = 2 * pair;
            let odd = even + 1;
            let cross = a[even] * ja[even] + a[odd] * ja[odd];
            assert!(cross.abs() < 1.0e-15, "pair {pair} cross = {cross:e}");
        }
    }
    assert!(symplectic_rotate_channels(&a, 33, 1.0).is_err());
}

#[test]
fn tanh_f64_as_f32_and_batch_pade10_match_tanh_bit_for_bit() {
    let check_scalar = |x: f64| {
        let want = tanh(x).map(|v| (v as f32).to_bits());
        let got = tanh_f64_as_f32(x).map(f32::to_bits);
        assert_eq!(got, want, "scalar tanh_f64_as_f32({x:?})");
    };
    let check_batch = |xs: [f64; 4]| {
        let want = if xs.iter().all(|v| v.is_finite()) {
            Some([
                (tanh(xs[0]).expect("finite") as f32).to_bits(),
                (tanh(xs[1]).expect("finite") as f32).to_bits(),
                (tanh(xs[2]).expect("finite") as f32).to_bits(),
                (tanh(xs[3]).expect("finite") as f32).to_bits(),
            ])
        } else {
            None
        };
        let got = tanh_f64_as_f32_4(xs).map(|out| out.map(f32::to_bits));
        assert_eq!(got, want, "batch tanh_f64_as_f32_4({xs:?})");
    };

    // Dense sweep across Padé [9/10] regime `[0, 2.25]`, boundary `2.25`, and
    // Cody–Waite regime `(2.25, 20)`.
    let mut grid = Vec::with_capacity(60_000);
    for i in 0..=45_000_u32 {
        let x = f64::from(i) * (2.25 / 45_000.0);
        grid.push(x);
        grid.push(-x);
    }
    for i in 1..=5_000_u32 {
        let x = 2.25 + f64::from(i) * (17.75 / 5_000.0);
        grid.push(x);
        grid.push(-x);
    }
    // Special values: ±0, subnormals, F32 min normal, exact regime transitions,
    // saturation threshold, huge values, and F32 rounding-tie neighborhoods.
    let mut special = vec![
        0.0,
        -0.0,
        f64::from(f32::MIN_POSITIVE) * 0.25,
        -f64::from(f32::MIN_POSITIVE) * 0.5,
        f64::from(f32::MIN_POSITIVE),
        2.0 * f64::from(f32::MIN_POSITIVE),
        1.0e-12,
        -1.0e-7,
        1.0 / 32.0,
        2.25_f64.next_down(),
        2.25,
        2.25_f64.next_up(),
        19.999_999,
        20.0_f64.next_down(),
        20.0,
        -20.0,
        1.0e100,
        -1.0e300,
    ];
    // Synthesize near-F32-tie inputs `x = atanh(tie)` across Padé & Estrin.
    for target in [0.001_f64, 0.25, 0.75, 0.978, 0.9999] {
        let base_bits = (target.to_bits() & !((1_u64 << 29) - 1)) | (1_u64 << 28);
        for delta in -4_i64..=4 {
            let y = f64::from_bits(base_bits.wrapping_add_signed(delta));
            if y > 0.0 && y < 1.0 {
                let x = y.atanh();
                special.push(x);
                special.push(-x);
            }
        }
    }
    for &x in &special {
        check_scalar(x);
    }
    for &x in &grid {
        check_scalar(x);
    }
    for &chunk in grid.as_chunks::<4>().0 {
        check_batch(chunk);
    }
    // All-zero / all-saturated fast exit, pure Padé, pure large, and mixed-regime 4-tuples.
    for batch in [
        [0.0, -0.0, 20.0, -1.0e100],
        [0.0, 0.0, 0.0, 0.0],
        [25.0, -25.0, 100.0, -100.0],
        [0.0, 1.0e-5, 0.5, 2.25],
        [-2.25, 1.0e-40, 0.0, 20.0],
        [0.001, 2.25_f64.next_up(), -0.5, 5.0],
        [-19.5, 0.0, 1.75, 25.0],
        [3.0, -4.5, 8.2, 15.0],
        [0.5, f64::NAN, 1.0, 2.0],
        [f64::INFINITY, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, f64::NEG_INFINITY],
    ] {
        check_batch(batch);
    }
}

#[test]
fn simd_4_lane_exp_expm1_tanh_sigmoid_and_tanh_f32_4_match_scalar_bit_for_bit() {
    let check_exp = |xs: [f64; 4]| {
        let want = [
            exp_neg(xs[0]),
            exp_neg(xs[1]),
            exp_neg(xs[2]),
            exp_neg(xs[3]),
        ];
        let want_opt = if want.iter().all(Option::is_some) {
            Some([
                want[0].expect("some").to_bits(),
                want[1].expect("some").to_bits(),
                want[2].expect("some").to_bits(),
                want[3].expect("some").to_bits(),
            ])
        } else {
            None
        };
        assert_eq!(
            exp_neg_4(xs).map(|v| v.map(f64::to_bits)),
            want_opt,
            "exp_neg_4({xs:?})"
        );
    };
    let check_expm1 = |xs: [f64; 4]| {
        let want = [
            expm1_neg(xs[0]),
            expm1_neg(xs[1]),
            expm1_neg(xs[2]),
            expm1_neg(xs[3]),
        ];
        let want_opt = if want.iter().all(Option::is_some) {
            Some([
                want[0].expect("some").to_bits(),
                want[1].expect("some").to_bits(),
                want[2].expect("some").to_bits(),
                want[3].expect("some").to_bits(),
            ])
        } else {
            None
        };
        assert_eq!(
            expm1_neg_4(xs).map(|v| v.map(f64::to_bits)),
            want_opt,
            "expm1_neg_4({xs:?})"
        );
    };
    let check_tanh = |xs: [f64; 4]| {
        let want = [tanh(xs[0]), tanh(xs[1]), tanh(xs[2]), tanh(xs[3])];
        let want_opt = if want.iter().all(Option::is_some) {
            Some([
                want[0].expect("some").to_bits(),
                want[1].expect("some").to_bits(),
                want[2].expect("some").to_bits(),
                want[3].expect("some").to_bits(),
            ])
        } else {
            None
        };
        assert_eq!(
            tanh_4(xs).map(|v| v.map(f64::to_bits)),
            want_opt,
            "tanh_4({xs:?})"
        );
    };
    let check_sigmoid = |xs: [f64; 4]| {
        let want = [
            sigmoid(xs[0]),
            sigmoid(xs[1]),
            sigmoid(xs[2]),
            sigmoid(xs[3]),
        ];
        let want_opt = if want.iter().all(Option::is_some) {
            Some([
                want[0].expect("some").to_bits(),
                want[1].expect("some").to_bits(),
                want[2].expect("some").to_bits(),
                want[3].expect("some").to_bits(),
            ])
        } else {
            None
        };
        assert_eq!(
            sigmoid_4(xs).map(|v| v.map(f64::to_bits)),
            want_opt,
            "sigmoid_4({xs:?})"
        );
    };
    let check_tanh_f32_4 = |xs: [f32; 4]| {
        let want = [
            tanh_f32(xs[0]),
            tanh_f32(xs[1]),
            tanh_f32(xs[2]),
            tanh_f32(xs[3]),
        ];
        let want_opt = if want.iter().all(Option::is_some) {
            Some([
                want[0].expect("some").to_bits(),
                want[1].expect("some").to_bits(),
                want[2].expect("some").to_bits(),
                want[3].expect("some").to_bits(),
            ])
        } else {
            None
        };
        assert_eq!(
            tanh_f32_4(xs).map(|v| v.map(f32::to_bits)),
            want_opt,
            "tanh_f32_4({xs:?})"
        );
    };

    // Mixed-regime tuples hitting every branch combination (zero, series,
    // reduced, subnormal, underflow, saturation, sign flips, and refusals).
    for xs in [
        [0.0, -0.0, 746.0, 1.0e100],
        [0.0, 0.01, 1.5, 745.1],
        [0.0, 0.0625, 0.0625_f64.next_down(), 700.0],
        [1.0e-12, 0.05, 1.0, 40.0],
        [0.0, 39.999, 40.0, 100.0],
        [0.1, 2.5, 10.0, 745.13],
        [0.0, -1.0e-300, 1.0, 2.0],
        [1.0, f64::NAN, 2.0, 3.0],
        [f64::INFINITY, 0.0, 1.0, 2.0],
    ] {
        check_exp(xs);
        check_expm1(xs);
    }

    for xs in [
        [0.0, -0.0, 20.0, -20.0],
        [1.0e-12, -1.0e-5, 0.03125_f64.next_down(), 0.03125],
        [-0.03125_f64.next_up(), 0.5, -2.5, 19.999],
        // Explicit `tanh_4` straddle across `ln(2)/2`, `1.0`, `20.0`, and `±0.0`.
        [0.4, -1.25, 0.999_999, 25.0],
        [-0.0, 0.35, -1.0, 19.5],
        [0.346_573_590_279_972_7, 1.0_f64.next_down(), 1.0, -20.0],
        [0.0, -0.0, 746.0, -746.0],
        [-700.0, -10.0, 10.0, 700.0],
        [-1.0e300, -1.0e-20, 1.0e-20, 1.0e300],
        [0.5, f64::NAN, -0.5, 1.0],
        [f64::NEG_INFINITY, 0.0, 0.0, 0.0],
    ] {
        check_tanh(xs);
        check_sigmoid(xs);
    }

    // Randomized broad sweep across all regimes for every 4-lane function.
    let mut state = 0xA11C_E5ED_C0DE_7777_u64;
    let mut next_u64 = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        state
    };
    for _ in 0..25_000 {
        let mut nonneg = [0.0_f64; 4];
        let mut signed = [0.0_f64; 4];
        let mut f32s = [0.0_f32; 4];
        for lane in 0..4 {
            let r = next_u64();
            let sign = if r & 1 == 0 { 1.0 } else { -1.0 };
            let exp_idx = ((r >> 1) % 40) as i32 - 30; // 2^-30 .. 2^9 (~512)
            let frac = ((r >> 11) as f64) / ((1_u64 << 53) as f64);
            let mag = (1.0 + frac) * 2.0_f64.powi(exp_idx);
            nonneg[lane] = if (r >> 7) & 15 == 0 { 0.0 } else { mag };
            signed[lane] = if (r >> 7) & 15 == 0 {
                sign * 0.0
            } else {
                sign * mag
            };
            f32s[lane] = signed[lane] as f32;
        }
        check_exp(nonneg);
        check_expm1(nonneg);
        check_tanh(signed);
        check_sigmoid(signed);
        check_tanh_f32_4(f32s);
    }
}
