// FrankenLife's fused controller kernel (plan §10.2, FL-030): every due
// organism's own dense tanh network, one threadgroup per organism, in the
// reference controller's declared arithmetic, bit for bit.
//
// The reference (flife-neural's ReferenceController) accumulates each neuron
// in F32, bias first and then the inputs in ascending index, rounding every
// product and every sum, with no fused multiply-add. Its hidden activation is
// flife-core's `numerics::tanh`, evaluated in F64 and rounded once to F32.
// Apple GPUs have no F64, so this file carries a software IEEE-754 binary64
// for finite operands, rounding to nearest with ties to even: Berkeley
// SoftFloat 3's add, subtract, multiply and pack routines, a long division,
// and the exact conversions. The activation below runs the reference
// algorithm operation for operation on it, so every intermediate is the
// CPU's value and the rounded result is the CPU's.

#include <metal_stdlib>
using namespace metal;

#pragma clang fp contract(off)

// ---------------------------------------------------------------------------
// Software binary64 on ulong bit patterns.

typedef ulong d64;

constant ulong D_SIGN = 0x8000000000000000ul;
constant ulong D_FRAC = 0x000FFFFFFFFFFFFFul;
constant ulong D_HIDDEN = 0x0010000000000000ul;
constant ulong D_QNAN = 0x7FF8000000000000ul;

static inline bool d_sign(d64 a) { return (a >> 63) != 0ul; }
static inline int d_exp(d64 a) { return int((a >> 52) & 0x7FFul); }
static inline ulong d_frac(d64 a) { return a & D_FRAC; }

// SoftFloat's packToF64UI: a hidden bit in `sig` carries into the exponent.
static inline d64 d_pack(bool sign, int exp, ulong sig) {
    return (sign ? D_SIGN : 0ul) + (ulong(uint(exp)) << 52) + sig;
}

// SoftFloat's softfloat_shiftRightJam64, for 0 < dist.
static inline ulong shift_right_jam(ulong a, uint dist) {
    return (dist < 63u) ? ((a >> dist) | ulong((a << (64u - dist)) != 0ul)) : ulong(a != 0ul);
}

// SoftFloat's softfloat_roundPackToF64, nearest-even: `sig` has its leading
// one at bit 62 and ten rounding bits; `exp` is one below the biased exponent.
static d64 round_pack(bool sign, int exp, ulong sig) {
    uint round_bits = uint(sig & 0x3FFul);
    if (uint(exp) >= 0x7FDu) {
        if (exp < 0) {
            sig = shift_right_jam(sig, uint(-exp));
            exp = 0;
            round_bits = uint(sig & 0x3FFul);
        } else if (exp > 0x7FD || sig + 0x200ul >= 0x8000000000000000ul) {
            return d_pack(sign, 0x7FF, 0ul);
        }
    }
    sig = (sig + 0x200ul) >> 10;
    if (round_bits == 0x200u) {
        sig &= ~1ul;
    }
    if (sig == 0ul) {
        exp = 0;
    }
    return d_pack(sign, exp, sig);
}

// SoftFloat's softfloat_normRoundPackToF64.
static d64 norm_round_pack(bool sign, int exp, ulong sig) {
    int shift = int(clz(sig)) - 1;
    exp -= shift;
    if (shift >= 10 && uint(exp) < 0x7FDu) {
        return d_pack(sign, sig != 0ul ? exp : 0, sig << uint(shift - 10));
    }
    return round_pack(sign, exp, sig << uint(shift));
}

// SoftFloat's softfloat_addMagsF64.
static d64 add_mags(d64 a, d64 b, bool sign) {
    int ea = d_exp(a);
    int eb = d_exp(b);
    ulong sa = d_frac(a);
    ulong sb = d_frac(b);
    int diff = ea - eb;
    int ez;
    ulong sz;
    if (diff == 0) {
        if (ea == 0) {
            return a + sb;
        }
        if (ea == 0x7FF) {
            return D_QNAN;
        }
        ez = ea;
        sz = (0x0020000000000000ul + sa + sb) << 9;
    } else {
        sa <<= 9;
        sb <<= 9;
        if (diff < 0) {
            if (eb == 0x7FF) {
                return D_QNAN;
            }
            ez = eb;
            sa = (ea != 0) ? sa + 0x2000000000000000ul : sa << 1;
            sa = shift_right_jam(sa, uint(-diff));
        } else {
            if (ea == 0x7FF) {
                return D_QNAN;
            }
            ez = ea;
            sb = (eb != 0) ? sb + 0x2000000000000000ul : sb << 1;
            sb = shift_right_jam(sb, uint(diff));
        }
        sz = 0x2000000000000000ul + sa + sb;
        if (sz < 0x4000000000000000ul) {
            ez -= 1;
            sz <<= 1;
        }
    }
    return round_pack(sign, ez, sz);
}

// SoftFloat's softfloat_subMagsF64.
static d64 sub_mags(d64 a, d64 b, bool sign) {
    int ea = d_exp(a);
    int eb = d_exp(b);
    ulong sa = d_frac(a);
    ulong sb = d_frac(b);
    int diff = ea - eb;
    if (diff == 0) {
        if (ea == 0x7FF) {
            return D_QNAN;
        }
        long sdiff = long(sa) - long(sb);
        if (sdiff == 0) {
            return 0ul;
        }
        if (ea != 0) {
            ea -= 1;
        }
        if (sdiff < 0) {
            sign = !sign;
            sdiff = -sdiff;
        }
        int shift = int(clz(ulong(sdiff))) - 11;
        int ez = ea - shift;
        if (ez < 0) {
            shift = ea;
            ez = 0;
        }
        return d_pack(sign, ez, ulong(sdiff) << uint(shift));
    }
    sa <<= 10;
    sb <<= 10;
    int ez;
    ulong sz;
    if (diff < 0) {
        sign = !sign;
        if (eb == 0x7FF) {
            return D_QNAN;
        }
        sa += (ea != 0) ? 0x4000000000000000ul : sa;
        sa = shift_right_jam(sa, uint(-diff));
        sb |= 0x4000000000000000ul;
        ez = eb;
        sz = sb - sa;
    } else {
        if (ea == 0x7FF) {
            return D_QNAN;
        }
        sb += (eb != 0) ? 0x4000000000000000ul : sb;
        sb = shift_right_jam(sb, uint(diff));
        sa |= 0x4000000000000000ul;
        ez = ea;
        sz = sa - sb;
    }
    return norm_round_pack(sign, ez - 1, sz);
}

static d64 d_add(d64 a, d64 b) {
    bool sa = d_sign(a);
    return (sa == d_sign(b)) ? add_mags(a, b, sa) : sub_mags(a, b, sa);
}

static d64 d_sub(d64 a, d64 b) {
    bool sa = d_sign(a);
    return (sa == d_sign(b)) ? sub_mags(a, b, sa) : add_mags(a, b, sa);
}

// SoftFloat's softfloat_normSubnormalF64Sig: the leading one to bit 52.
static inline void norm_subnormal(thread int &exp, thread ulong &sig) {
    int shift = int(clz(sig)) - 11;
    exp = 1 - shift;
    sig <<= uint(shift);
}

// SoftFloat's f64_mul with softfloat_mul64To128.
static d64 d_mul(d64 a, d64 b) {
    bool sz = d_sign(a) != d_sign(b);
    int ea = d_exp(a);
    int eb = d_exp(b);
    ulong sa = d_frac(a);
    ulong sb = d_frac(b);
    if (ea == 0x7FF || eb == 0x7FF) {
        return D_QNAN;
    }
    if (ea == 0) {
        if (sa == 0ul) {
            return d_pack(sz, 0, 0ul);
        }
        norm_subnormal(ea, sa);
    }
    if (eb == 0) {
        if (sb == 0ul) {
            return d_pack(sz, 0, 0ul);
        }
        norm_subnormal(eb, sb);
    }
    int ez = ea + eb - 0x3FF;
    sa = (sa | D_HIDDEN) << 10;
    sb = (sb | D_HIDDEN) << 11;
    ulong a32 = sa >> 32;
    ulong a0 = sa & 0xFFFFFFFFul;
    ulong b32 = sb >> 32;
    ulong b0 = sb & 0xFFFFFFFFul;
    ulong low = a0 * b0;
    ulong mid1 = a32 * b0;
    ulong mid = mid1 + a0 * b32;
    ulong high = a32 * b32;
    high += (ulong(mid < mid1) << 32) | (mid >> 32);
    mid <<= 32;
    low += mid;
    high += ulong(low < mid);
    ulong sig = high | ulong(low != 0ul);
    if (sig < 0x4000000000000000ul) {
        ez -= 1;
        sig <<= 1;
    }
    return round_pack(sz, ez, sig);
}

// Division by restoring long division: 63 quotient bits with the leading one
// at bit 62, and a nonzero remainder as the sticky bit.
static d64 d_div(d64 a, d64 b) {
    bool sz = d_sign(a) != d_sign(b);
    int ea = d_exp(a);
    int eb = d_exp(b);
    ulong sa = d_frac(a);
    ulong sb = d_frac(b);
    if (ea == 0x7FF || eb == 0x7FF) {
        return D_QNAN;
    }
    if (eb == 0) {
        if (sb == 0ul) {
            return (ea == 0 && sa == 0ul) ? D_QNAN : d_pack(sz, 0x7FF, 0ul);
        }
        norm_subnormal(eb, sb);
    }
    if (ea == 0) {
        if (sa == 0ul) {
            return d_pack(sz, 0, 0ul);
        }
        norm_subnormal(ea, sa);
    }
    int ez = ea - eb + 0x3FE;
    sa |= D_HIDDEN;
    sb |= D_HIDDEN;
    if (sa < sb) {
        ez -= 1;
        sa <<= 1;
    }
    ulong r = sa;
    ulong q = 0ul;
    for (int i = 0; i < 63; ++i) {
        q <<= 1;
        if (r >= sb) {
            r -= sb;
            q |= 1ul;
        }
        r <<= 1;
    }
    q |= ulong(r != 0ul);
    return round_pack(sz, ez, q);
}

// Rust's `f64::round`: half away from zero.
static d64 d_round(d64 a) {
    int e = d_exp(a) - 1023;
    if (e >= 52) {
        return a;
    }
    if (e < -1) {
        return a & D_SIGN;
    }
    if (e == -1) {
        return (a & D_SIGN) | 0x3FF0000000000000ul;
    }
    ulong mask = D_FRAC >> uint(e);
    if ((a & mask) == 0ul) {
        return a;
    }
    ulong bump = (1ul << 51) >> uint(e);
    return (a + bump) & ~mask;
}

// `as i32` of an integral value below 2^31 in magnitude.
static int d_to_int(d64 a) {
    int e = d_exp(a) - 1023;
    if (e < 0) {
        return 0;
    }
    int v = int((d_frac(a) | D_HIDDEN) >> uint(52 - e));
    return d_sign(a) ? -v : v;
}

static inline d64 power_of_two(int k) {
    return ulong(uint(k + 1023)) << 52;
}

// SoftFloat's f32_to_f64: exact.
static d64 d_from_f32(float x) {
    uint u = as_type<uint>(x);
    bool s = (u >> 31) != 0u;
    int e = int((u >> 23) & 0xFFu);
    uint f = u & 0x7FFFFFu;
    if (e == 0xFF) {
        return D_QNAN;
    }
    if (e == 0) {
        if (f == 0u) {
            return d_pack(s, 0, 0ul);
        }
        int shift = int(clz(f)) - 8;
        e = 1 - shift;
        f = (f << uint(shift)) & 0x7FFFFFu;
    }
    return d_pack(s, e + 0x380, ulong(f) << 29);
}

static inline uint shift_right_jam32(uint a, uint dist) {
    return (dist < 31u) ? ((a >> dist) | uint((a << (32u - dist)) != 0u)) : uint(a != 0u);
}

// SoftFloat's f64_to_f32 with softfloat_roundPackToF32, nearest-even.
static float d_to_f32(d64 a) {
    bool s = d_sign(a);
    int e = d_exp(a);
    ulong f = d_frac(a);
    uint sign = s ? 0x80000000u : 0u;
    if (e == 0x7FF) {
        return as_type<float>(0x7FC00000u);
    }
    uint sig = uint((f >> 22) | ulong((f & 0x3FFFFFul) != 0ul));
    if ((uint(e) | sig) == 0u) {
        return as_type<float>(sign);
    }
    e -= 0x381;
    sig |= 0x40000000u;
    uint round_bits = sig & 0x7Fu;
    if (uint(e) >= 0xFDu) {
        if (e < 0) {
            sig = shift_right_jam32(sig, uint(-e));
            e = 0;
            round_bits = sig & 0x7Fu;
        } else if (e > 0xFD || sig + 0x40u >= 0x80000000u) {
            return as_type<float>(sign | 0x7F800000u);
        }
    }
    sig = (sig + 0x40u) >> 7;
    if (round_bits == 0x40u) {
        sig &= ~1u;
    }
    if (sig == 0u) {
        e = 0;
    }
    return as_type<float>(sign + (uint(e) << 23) + sig);
}

// ---------------------------------------------------------------------------
// flife-core's numerics, operation for operation, for finite arguments.

constant ulong ONE = 0x3FF0000000000000ul;
constant ulong TWO = 0x4000000000000000ul;
constant ulong HALF = 0x3FE0000000000000ul;
constant ulong REDUCED = 0x3FB0000000000000ul;          // 1/16
constant ulong TANH_SATURATES = 0x4034000000000000ul;   // 20
constant ulong EXPM1_SATURATES = 0x4044000000000000ul;  // 40
constant ulong EXP_UNDERFLOWS = 0x4087500000000000ul;   // 746
constant ulong LN_2 = 0x3FE62E42FEFA39EFul;
constant ulong LN2_HI = 0x3FE62E42FEE00000ul;
constant ulong LN2_LO = 0x3DEA39EF35793C76ul;
// `f64::from(k)` for k = 0..16.
constant ulong INTEGER[17] = {
    0x0000000000000000ul, 0x3FF0000000000000ul, 0x4000000000000000ul, 0x4008000000000000ul,
    0x4010000000000000ul, 0x4014000000000000ul, 0x4018000000000000ul, 0x401C000000000000ul,
    0x4020000000000000ul, 0x4022000000000000ul, 0x4024000000000000ul, 0x4026000000000000ul,
    0x4028000000000000ul, 0x402A000000000000ul, 0x402C000000000000ul, 0x402E000000000000ul,
    0x4030000000000000ul,
};

// Nonnegative binary64 values order as their bit patterns, so the
// comparisons below compare bits.

// numerics::exp_series.
static d64 exp_series(d64 y, int terms) {
    d64 series = ONE;
    for (int term = terms; term >= 1; --term) {
        series = d_sub(ONE, d_mul(d_div(y, INTEGER[term]), series));
    }
    return series;
}

// numerics::exp_neg for finite x >= 0.
static d64 exp_neg(d64 x) {
    if (x <= REDUCED) {
        return exp_series(x, 12);
    }
    if (x >= EXP_UNDERFLOWS) {
        return 0ul;
    }
    d64 n = d_round(d_div(x, LN_2));
    d64 r = d_sub(d_sub(x, d_mul(n, LN2_HI)), d_mul(n, LN2_LO));
    d64 scaled = exp_series(r, 15);
    int k = d_to_int(n);
    int first = min(k, 1000);
    return d_mul(d_mul(scaled, power_of_two(-first)), power_of_two(first - k));
}

// numerics::expm1_neg for finite x >= 0.
static d64 expm1_neg(d64 x) {
    if (x >= EXPM1_SATURATES) {
        return ONE;
    }
    if (x >= LN_2) {
        return d_sub(ONE, exp_neg(x));
    }
    d64 y = x;
    int halvings = 0;
    while (y > REDUCED) {
        y = d_mul(y, HALF);
        halvings += 1;
    }
    d64 series = ONE;
    for (int term = 14; term >= 2; --term) {
        series = d_sub(ONE, d_mul(d_div(y, INTEGER[term]), series));
    }
    d64 m = d_mul(y, series);
    for (int i = 0; i < halvings; ++i) {
        m = d_mul(m, d_sub(TWO, m));
    }
    return m;
}

// `numerics::tanh(f64::from(z)) as f32` for finite z.
static float tanh_reference(float z) {
    d64 x = d_from_f32(z);
    d64 magnitude = x & ~D_SIGN;
    d64 value;
    if (magnitude >= TANH_SATURATES) {
        value = ONE;
    } else if (magnitude >= ONE) {
        d64 d = exp_neg(d_mul(TWO, magnitude));
        value = d_sub(ONE, d_div(d_mul(TWO, d), d_add(ONE, d)));
    } else {
        d64 m = expm1_neg(d_mul(TWO, magnitude));
        value = d_div(m, d_sub(TWO, m));
    }
    if (d_sign(x)) {
        value ^= D_SIGN;
    }
    return d_to_f32(value);
}

// ---------------------------------------------------------------------------
// The fast path: the same activation from hardware F32 arithmetic in
// float-float (an unevaluated sum hi + lo, about 48 bits), accepted only
// where it provably rounds as the reference does.
//
// The reference's F64 value T is within 2^-51 of tanh, and the float-float
// value V within about 2^-44. Every value within 2^-38 |V| of V rounds to
// the same F32 unless an F32 rounding midpoint lies that close; then the
// lane falls back to the software binary64 above. The exhaustive test checks
// the composite, fast path and fallback together, at every F32.

struct ff {
    float hi;
    float lo;
};

static inline ff ff_make(float hi, float lo) {
    ff r;
    r.hi = hi;
    r.lo = lo;
    return r;
}

// Knuth's two-sum: s + e = a + b exactly.
static inline ff two_sum(float a, float b) {
    float s = a + b;
    float bb = s - a;
    float e = (a - (s - bb)) + (b - bb);
    return ff_make(s, e);
}

// Dekker's fast two-sum, for |a| >= |b|.
static inline ff quick_two_sum(float a, float b) {
    float s = a + b;
    float e = b - (s - a);
    return ff_make(s, e);
}

// p + e = a b exactly, by one fused multiply-add.
static inline ff two_prod(float a, float b) {
    float p = a * b;
    float e = fma(a, b, -p);
    return ff_make(p, e);
}

static inline ff ff_add(ff a, ff b) {
    ff s = two_sum(a.hi, b.hi);
    ff t = two_sum(a.lo, b.lo);
    s.lo = s.lo + t.hi;
    s = quick_two_sum(s.hi, s.lo);
    s.lo = s.lo + t.lo;
    return quick_two_sum(s.hi, s.lo);
}

static inline ff ff_neg(ff a) {
    return ff_make(-a.hi, -a.lo);
}

static inline ff ff_mul(ff a, ff b) {
    ff p = two_prod(a.hi, b.hi);
    float cross = a.hi * b.lo;
    cross = cross + a.lo * b.hi;
    p.lo = p.lo + cross;
    return quick_two_sum(p.hi, p.lo);
}

static inline ff ff_div(ff a, ff b) {
    float q1 = a.hi / b.hi;
    ff r = ff_add(a, ff_neg(ff_mul(b, ff_make(q1, 0.0f))));
    float q2 = r.hi / b.hi;
    r = ff_add(r, ff_neg(ff_mul(b, ff_make(q2, 0.0f))));
    float q3 = r.hi / b.hi;
    return ff_add(quick_two_sum(q1, q2), ff_make(q3, 0.0f));
}

// tanh x = x P(x^2) for |x| < 1/2: P's Taylor coefficients, (hi, lo).
constant float2 TANH_SERIES[16] = {
    {as_type<float>(0x3F800000u), as_type<float>(0x00000000u)},
    {as_type<float>(0xBEAAAAABu), as_type<float>(0x322AAAABu)},
    {as_type<float>(0x3E088889u), as_type<float>(0xB1EEEEEFu)},
    {as_type<float>(0xBD5D0DD1u), as_type<float>(0x300BC8BDu)},
    {as_type<float>(0x3CB327A4u), as_type<float>(0x3002C110u)},
    {as_type<float>(0xBC11371Bu), as_type<float>(0x2FA193DCu)},
    {as_type<float>(0x3B6B69E8u), as_type<float>(0x2EE157DEu)},
    {as_type<float>(0xBABED1B2u), as_type<float>(0xADA56EBCu)},
    {as_type<float>(0x3A1AAC12u), as_type<float>(0x2D803674u)},
    {as_type<float>(0xB97ABEBCu), as_type<float>(0x2CCB2E9Au)},
    {as_type<float>(0x38CB3F0Cu), as_type<float>(0x2C2FCAFBu)},
    {as_type<float>(0xB824BEC7u), as_type<float>(0xABEA2526u)},
    {as_type<float>(0x37858997u), as_type<float>(0xAB4659FBu)},
    {as_type<float>(0xB6D87B97u), as_type<float>(0x2A411DB1u)},
    {as_type<float>(0x362F796Du), as_type<float>(0x290E9CB7u)},
    {as_type<float>(0xB58E3BF0u), as_type<float>(0x28D47C7Eu)},
};

// e^(-r) = sum (-1)^n r^n / n!, n = 0..13, for |r| <= ln2/2, (hi, lo).
constant float2 EXP_SERIES[14] = {
    {as_type<float>(0x3F800000u), as_type<float>(0x00000000u)},
    {as_type<float>(0xBF800000u), as_type<float>(0x00000000u)},
    {as_type<float>(0x3F000000u), as_type<float>(0x00000000u)},
    {as_type<float>(0xBE2AAAABu), as_type<float>(0x31AAAAABu)},
    {as_type<float>(0x3D2AAAABu), as_type<float>(0xB0AAAAABu)},
    {as_type<float>(0xBC088889u), as_type<float>(0x2FEEEEEFu)},
    {as_type<float>(0x3AB60B61u), as_type<float>(0xAE13E93Fu)},
    {as_type<float>(0xB9500D01u), as_type<float>(0x2C3FCBFDu)},
    {as_type<float>(0x37D00D01u), as_type<float>(0xAABFCBFDu)},
    {as_type<float>(0xB638EF1Du), as_type<float>(0xA92AD8E6u)},
    {as_type<float>(0x3493F27Eu), as_type<float>(0xA808760Au)},
    {as_type<float>(0xB2D7322Bu), as_type<float>(0xA5FEA89Cu)},
    {as_type<float>(0x310F76C7u), as_type<float>(0x24FF8D89u)},
    {as_type<float>(0xAF309231u), as_type<float>(0x22C5792Fu)},
};

// ln 2 = L1 + L2 + L3 to 2^-69: L1 has 16 significant bits, so k L1 is
// exact for k < 256.
constant float LN2_1 = as_type<float>(0x3F317200u);
constant float LN2_2 = as_type<float>(0x35BFBE8Eu);
constant float LN2_3 = as_type<float>(0x29779ABDu);
constant float INV_LN2 = as_type<float>(0x3FB8AA3Bu);

constant uint FAST_IDENTITY_BELOW = 0x39000000u;  // 2^-13
constant uint FAST_SERIES_BELOW = 0x3F000000u;    // 1/2
constant uint SATURATES_AT = 0x41A00000u;         // 20
constant float GUARD = as_type<float>(0x2C800000u);  // 2^-38

// tanh a for 2^-13 <= a < 1/2.
static ff tanh_series(float a) {
    ff s = two_prod(a, a);
    ff acc = ff_make(TANH_SERIES[15].x, TANH_SERIES[15].y);
    for (int n = 14; n >= 0; --n) {
        acc = ff_add(ff_mul(acc, s), ff_make(TANH_SERIES[n].x, TANH_SERIES[n].y));
    }
    return ff_mul(acc, ff_make(a, 0.0f));
}

// tanh a = 1 - 2e/(1 + e), e = e^(-2a), for 1/2 <= a < 20.
static ff tanh_exp(float a) {
    float t = a + a;
    float k = rint(t * INV_LN2);
    // t - k L1 is exact (Sterbenz); then the rest of k ln 2 in float-float.
    ff r = ff_make(t - k * LN2_1, 0.0f);
    r = ff_add(r, ff_neg(two_prod(k, LN2_2)));
    r = ff_add(r, ff_make(-(k * LN2_3), 0.0f));
    ff acc = ff_make(EXP_SERIES[13].x, EXP_SERIES[13].y);
    for (int n = 12; n >= 0; --n) {
        acc = ff_add(ff_mul(acc, r), ff_make(EXP_SERIES[n].x, EXP_SERIES[n].y));
    }
    int shift = -int(k);
    ff e = ff_make(ldexp(acc.hi, shift), ldexp(acc.lo, shift));
    ff fraction = ff_div(ff_make(e.hi + e.hi, e.lo + e.lo), ff_add(ff_make(1.0f, 0.0f), e));
    return ff_add(ff_make(1.0f, 0.0f), ff_neg(fraction));
}

// The F32 that every value within GUARD |v| of the positive normal v
// rounds to, if there is one.
static inline bool settle(ff v, thread float &out) {
    uint b = as_type<uint>(v.hi);
    float ulp = as_type<float>((b & 0x7F800000u) - (23u << 23));
    float above = 0.5f * ulp;
    float below = ((b & 0x007FFFFFu) == 0u) ? 0.25f * ulp : above;
    float margin = v.hi * GUARD;
    out = v.hi;
    return (v.lo >= 0.0f) ? (v.lo + margin < above) : (margin - v.lo < below);
}

// `numerics::tanh(f64::from(z)) as f32` for finite z: the fast path where it
// settles, the software binary64 elsewhere. `fallbacks` counts the latter.
static float activation(float z, thread uint &fallbacks) {
    uint magnitude = as_type<uint>(z) & 0x7FFFFFFFu;
    float a = as_type<float>(magnitude);
    float value;
    bool settled;
    if (magnitude >= SATURATES_AT) {
        value = 1.0f;
        settled = true;
    } else if (magnitude < FAST_IDENTITY_BELOW) {
        // tanh a = a (1 - a^2/3 + ...), and a^2/3 < 2^-27 rounds away.
        value = a;
        settled = true;
    } else if (magnitude < FAST_SERIES_BELOW) {
        settled = settle(tanh_series(a), value);
    } else {
        settled = settle(tanh_exp(a), value);
    }
    if (!settled) {
        fallbacks += 1u;
        return tanh_reference(z);
    }
    return (as_type<uint>(z) >> 31) != 0u ? -value : value;
}

// ---------------------------------------------------------------------------
// Kernels.

constant uint SENSORS = 104;
constant uint ACTIONS = 36;
constant uint WIDEST = 128;

// The organism's word in `faults`: a pre-activation or output that is not
// finite, or arithmetic near the subnormal range.
constant uint FAULT_NONFINITE = 1u;
constant uint FAULT_TINY = 2u;

// The F32 bits of 2^-102, as magnitudes order. This device flushes
// subnormal F32 results to zero, and the reference does not. A sum of two
// normal F32s of magnitude at least 2^-102 is a multiple of 2^-125, so it is
// zero or normal; a product at least 2^-102 is normal. A step that could
// reach a subnormal therefore has a nonzero operand or product below 2^-102,
// and any such step defers the organism to the reference.
constant uint TINY_BITS = 0x0C800000u;

// One threadgroup per organism, one thread per output neuron in turn. `jobs`
// holds four words per organism: the offset of its parameters in `params`,
// its hidden-layer count (1 or 2) and its hidden widths. A nonzero word in
// `faults` defers the organism to the host, which evaluates it on the
// reference controller: for its error, or for its exact value.
kernel void controller_forward(
    device const float *params [[buffer(0)]],
    device const float *inputs [[buffer(1)]],
    device const uint *jobs [[buffer(2)]],
    device float *outputs [[buffer(3)]],
    device uint *faults [[buffer(4)]],
    uint organism [[threadgroup_position_in_grid]],
    uint lane [[thread_position_in_threadgroup]],
    uint lanes [[threads_per_threadgroup]])
{
    threadgroup float sensed[SENSORS];
    threadgroup float first[WIDEST];
    threadgroup float second[WIDEST];
    threadgroup atomic_uint fault;
    uint fallbacks = 0u;
    if (lane == 0u) {
        atomic_store_explicit(&fault, 0u, memory_order_relaxed);
    }
    for (uint i = lane; i < SENSORS; i += lanes) {
        sensed[i] = inputs[organism * SENSORS + i];
    }
    threadgroup_barrier(mem_flags::mem_threadgroup);

    uint offset = jobs[organism * 4u];
    uint hidden = jobs[organism * 4u + 1u];
    uint widths[2] = {jobs[organism * 4u + 2u], jobs[organism * 4u + 3u]};
    threadgroup const float *source = sensed;
    uint fan_in = SENSORS;
    for (uint layer = 0u; layer <= hidden; ++layer) {
        bool last = layer == hidden;
        uint fan_out = last ? ACTIONS : widths[layer];
        threadgroup float *target = (layer == 0u) ? first : second;
        device const float *weights = params + offset;
        device const float *biases = weights + fan_in * fan_out;
        for (uint o = lane; o < fan_out; o += lanes) {
            device const float *row = weights + o * fan_in;
            float z = biases[o];
            bool tiny = false;
            for (uint i = 0u; i < fan_in; ++i) {
                float w = row[i];
                float x = source[i];
                float product = w * x;
                uint wb = as_type<uint>(w) & 0x7FFFFFFFu;
                uint xb = as_type<uint>(x) & 0x7FFFFFFFu;
                uint pb = as_type<uint>(product) & 0x7FFFFFFFu;
                uint zb = as_type<uint>(z) & 0x7FFFFFFFu;
                tiny = tiny || (zb - 1u < TINY_BITS - 1u)
                    || (pb < TINY_BITS && wb != 0u && xb != 0u);
                z = z + product;
            }
            if (tiny) {
                atomic_fetch_or_explicit(&fault, FAULT_TINY, memory_order_relaxed);
            }
            if (!isfinite(z)) {
                atomic_fetch_or_explicit(&fault, FAULT_NONFINITE, memory_order_relaxed);
            }
            if (last) {
                outputs[organism * ACTIONS + o] = z;
            } else {
                target[o] = isfinite(z) ? activation(z, fallbacks) : 0.0f;
            }
        }
        offset += fan_in * fan_out + fan_out;
        fan_in = fan_out;
        threadgroup_barrier(mem_flags::mem_threadgroup);
        source = target;
    }
    if (lane == 0u) {
        faults[organism] = atomic_load_explicit(&fault, memory_order_relaxed);
    }
}

// The activation alone: out[i] is the F32 bits of the activation of the F32
// whose bits are base[0] + i, the forward pass's (base[1] = 0) or the
// software binary64's alone (base[1] = 1). `fallbacks` counts the forward
// pass's fallbacks to the software binary64.
kernel void activation_bits(
    device const uint *base [[buffer(0)]],
    device uint *out [[buffer(1)]],
    device atomic_uint *fallbacks [[buffer(2)]],
    uint index [[thread_position_in_grid]])
{
    float z = as_type<float>(base[0] + index);
    if (base[1] != 0u) {
        out[index] = as_type<uint>(tanh_reference(z));
        return;
    }
    uint fell = 0u;
    out[index] = as_type<uint>(activation(z, fell));
    if (fell != 0u) {
        atomic_fetch_add_explicit(fallbacks, fell, memory_order_relaxed);
    }
}

// The binary64 operations alone. Each operand pair is four words (a low,
// a high, b low, b high); each result row is twelve: add, sub, mul, div and
// round(a) as word pairs, then the F32 rounding of a and of b.
kernel void binary64_ops(
    device const uint *operands [[buffer(0)]],
    device uint *out [[buffer(1)]],
    uint index [[thread_position_in_grid]])
{
    d64 a = ulong(operands[index * 4u]) | (ulong(operands[index * 4u + 1u]) << 32);
    d64 b = ulong(operands[index * 4u + 2u]) | (ulong(operands[index * 4u + 3u]) << 32);
    d64 results[5] = {d_add(a, b), d_sub(a, b), d_mul(a, b), d_div(a, b), d_round(a)};
    for (uint k = 0u; k < 5u; ++k) {
        out[index * 12u + 2u * k] = uint(results[k] & 0xFFFFFFFFul);
        out[index * 12u + 2u * k + 1u] = uint(results[k] >> 32);
    }
    out[index * 12u + 10u] = as_type<uint>(d_to_f32(a));
    out[index * 12u + 11u] = as_type<uint>(d_to_f32(b));
}

// The hardware F32 operations the kernel relies on: per pair, the product,
// the sum, and the fused multiply-add giving the product's rounding error.
kernel void binary32_ops(
    device const float *operands [[buffer(0)]],
    device float *out [[buffer(1)]],
    uint index [[thread_position_in_grid]])
{
    float a = operands[index * 2u];
    float b = operands[index * 2u + 1u];
    float product = a * b;
    float sum = a + b;
    out[index * 3u] = product;
    out[index * 3u + 1u] = sum;
    out[index * 3u + 2u] = fma(a, b, -product);
}
