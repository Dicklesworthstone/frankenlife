// FL-032: what phase 8 needs of flife-core beyond softfloat64.metal, after it
// and philox.metal: numerics::exp for either sign, ln and sin_cos_turns,
// rng's open_unit and box_muller, and binary16's conversions, operation for
// operation on software binary64.

constant ulong EXP_OVERFLOWS = 0x4086300000000000ul;    // 710
constant ulong SQRT_2 = 0x3FF6A09E667F3BCDul;
constant ulong SUBNORMAL_SCALE = 0x4350000000000000ul;  // 2^54
constant ulong FRAC_PI_2 = 0x3FF921FB54442D18ul;
constant ulong UNIT = 0x3CB0000000000000ul;             // 2^-52
constant ulong THREE = 0x4008000000000000ul;
constant ulong FOUR = 0x4010000000000000ul;
constant ulong MINUS_TWO = 0xC000000000000000ul;

// The exact binary64 of `v < 2^53`.
static d64 d_from_u53(ulong v) {
    if (v == 0ul) {
        return 0ul;
    }
    uint top = 63u - uint(clz(v));
    return (ulong(top + 1023u) << 52) | ((v << (52u - top)) & D_FRAC);
}

// The exact binary64 of `v`, `|v| < 2^31`.
static d64 d_from_int(int v) {
    d64 magnitude = d_from_u53(ulong(v < 0 ? -v : v));
    return v < 0 ? (magnitude | D_SIGN) : magnitude;
}

// numerics::exp for finite x: false where it returns None (an overflowing
// result), else the value in `value`.
static bool exp_signed(d64 x, thread d64 &value) {
    if (d_sign(x) || (x << 1) == 0ul) {
        // exp_neg(-x); -(+0) is -0, whose series is +0's.
        d64 negated = x ^ D_SIGN;
        value = exp_neg((negated << 1) == 0ul ? 0ul : negated);
        return true;
    }
    if (x >= EXP_OVERFLOWS) {
        return false;
    }
    if (x <= REDUCED) {
        value = exp_series(x ^ D_SIGN, 12);
        return true;
    }
    d64 n = d_round(d_div(x, LN_2));
    d64 r = d_sub(d_sub(x, d_mul(n, LN2_HI)), d_mul(n, LN2_LO));
    d64 scaled = exp_series(r ^ D_SIGN, 15);
    int k = d_to_int(n);
    int first = min(k, 1000);
    d64 result = d_mul(d_mul(scaled, power_of_two(first)), power_of_two(k - first));
    if (!d_finite(result)) {
        return false;
    }
    value = result;
    return true;
}

// numerics::log_parts for finite x > 0: `ln m`, with x = m 2^n and m in
// (sqrt(2)/2, sqrt(2)], and n in `exponent`.
static d64 log_parts(d64 x, thread int &exponent) {
    d64 normal = x;
    int adjust = 0;
    if (d_exp(x) == 0) {
        normal = d_mul(x, SUBNORMAL_SCALE);
        adjust = -54;
    }
    int n = d_exp(normal) - 1023 + adjust;
    d64 mantissa = d_frac(normal) | ONE;
    if (mantissa > SQRT_2) {
        mantissa = d_mul(mantissa, HALF);
        n += 1;
    }
    d64 z = d_div(d_sub(mantissa, ONE), d_add(mantissa, ONE));
    d64 z2 = d_mul(z, z);
    d64 series = 0ul;
    for (int k = 12; k >= 1; --k) {
        series = d_add(d_div(ONE, d_from_u32(uint(2 * k + 1))), d_mul(z2, series));
    }
    exponent = n;
    return d_mul(TWO, d_add(z, d_mul(z, d_mul(z2, series))));
}

// numerics::ln for finite x > 0.
static d64 d_ln(d64 x) {
    int exponent = 0;
    d64 natural = log_parts(x, exponent);
    d64 n = d_from_int(exponent);
    return d_add(d_mul(n, LN2_HI), d_add(d_mul(n, LN2_LO), natural));
}

// numerics::quarter_turn_kernel: (sin x, cos x) for x = (pi/2) r, 0 <= r <= 1/2.
static void quarter_turn(d64 r, thread d64 &sine, thread d64 &cosine) {
    d64 x = d_mul(r, FRAC_PI_2);
    d64 x2 = d_mul(x, x);
    d64 s = ONE;
    d64 c = ONE;
    for (uint k = 9u; k >= 1u; --k) {
        s = d_sub(ONE, d_mul(d_div(x2, d_from_u32((2u * k) * (2u * k + 1u))), s));
        c = d_sub(ONE, d_mul(d_div(x2, d_from_u32((2u * k - 1u) * (2u * k))), c));
    }
    sine = d_mul(x, s);
    cosine = c;
}

// numerics::sin_cos_turns for 0 <= u < 1: (sin 2 pi u, cos 2 pi u).
static void sin_cos_turns(d64 u, thread d64 &sine, thread d64 &cosine) {
    d64 quarters = d_mul(FOUR, u);
    d64 quadrant = d_floor(quarters);
    d64 r = d_sub(quarters, quadrant);
    d64 s;
    d64 c;
    if (r <= HALF) {
        quarter_turn(r, s, c);
    } else {
        quarter_turn(d_sub(ONE, r), c, s);
    }
    switch (d_to_int(quadrant)) {
        case 0:
            sine = s;
            cosine = c;
            break;
        case 1:
            sine = c;
            cosine = s ^ D_SIGN;
            break;
        case 2:
            sine = s ^ D_SIGN;
            cosine = c ^ D_SIGN;
            break;
        default:
            sine = c ^ D_SIGN;
            cosine = s;
            break;
    }
}

// rng::open_unit: `(k + 1/2) 2^-52` for the leading 52 bits `k`, exactly.
static d64 d_open_unit(ulong word) {
    return d_mul(d_add(d_from_u53(word >> 12), HALF), UNIT);
}

// rng::box_muller: component 0 (`rho cos 2 pi u2`) and component 1
// (`rho sin 2 pi u2`).
static void box_muller(ulong first, ulong second, thread d64 &zero, thread d64 &one) {
    d64 radius = d_sqrt(d_mul(MINUS_TWO, d_ln(d_open_unit(first))));
    d64 s;
    d64 c;
    sin_cos_turns(d_half_open_unit(second), s, c);
    zero = d_mul(radius, c);
    one = d_mul(radius, s);
}

// binary16's round_shift: `value >> shift` to nearest, ties to even, for
// 1 <= shift <= 53.
static ulong round_shift(ulong value, uint shift) {
    ulong kept = value >> shift;
    ulong rest = value & ((1ul << shift) - 1ul);
    ulong midpoint = 1ul << (shift - 1u);
    return (rest > midpoint || (rest == midpoint && (kept & 1ul) == 1ul)) ? kept + 1ul : kept;
}

// F16::from_f64: x rounded once to binary16, ties to even; the bits.
static uint d_to_f16_bits(d64 x) {
    uint sign = uint((x >> 48) & 0x8000ul);
    int biased = int((x >> 52) & 0x7FFul);
    ulong fraction = x & D_FRAC;
    if (biased == 0x7FF) {
        return sign | (fraction == 0ul ? 0x7C00u : 0x7E00u);
    }
    if (biased == 0) {
        return sign;
    }
    int exponent = biased - 1023;
    if (exponent >= 16) {
        return sign | 0x7C00u;
    }
    ulong significand = fraction | (1ul << 52);
    if (exponent >= -14) {
        ulong kept = round_shift(significand, 42u);
        if (kept == (1ul << 11)) {
            kept = 1ul << 10;
            exponent += 1;
        }
        if (exponent >= 16) {
            return sign | 0x7C00u;
        }
        return sign | (uint(exponent + 15) << 10) | uint(kept & 0x3FFul);
    }
    int shift = 28 - exponent;
    if (shift >= 54) {
        return sign;
    }
    return sign | uint(round_shift(significand, uint(shift)));
}

// F16::to_f32: the binary32 bits of binary16 bits `h`, exactly.
static uint f16_to_f32_bits(uint h) {
    uint sign = (h & 0x8000u) << 16;
    uint exponent = (h & 0x7C00u) >> 10;
    uint fraction = h & 0x03FFu;
    uint magnitude;
    if (exponent == 0u) {
        // fraction 2^-24: normalized at the fraction's top bit.
        if (fraction == 0u) {
            magnitude = 0u;
        } else {
            uint top = 31u - clz(fraction);
            magnitude = ((top + 103u) << 23) | ((fraction << (23u - top)) & 0x7FFFFFu);
        }
    } else if (exponent == 0x1Fu) {
        magnitude = 0x7F800000u | (fraction << 13);
    } else {
        magnitude = ((exponent + 112u) << 23) | (fraction << 13);
    }
    return sign | magnitude;
}
