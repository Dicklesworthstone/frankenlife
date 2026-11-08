//! Host-independent elementary functions (plan A.1, C.11): pinned bits,
//! distance from correctly rounded values, agreement with the platform
//! `libm`, and the domain and symmetry each function promises.

use flife_core::numerics::{exp_neg, expm1_neg, sigmoid, tanh};

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
