// Software IEEE-754 binary64 for FrankenLife's Metal kernels (FL-030,
// FL-031), compiled ahead of each kernel file. Apple GPUs have no F64, so
// values are ulong bit patterns, rounding to nearest with ties to even:
// Berkeley SoftFloat 3's add, subtract, multiply and pack routines, a long
// division, the exact conversions, and flife-core's numerics operation for
// operation, for finite operands. Every result is the CPU's, bit for bit.
//
// F32 values cross as bit patterns (`d_from_f32_bits`, `d_to_f32_bits`):
// the device flushes subnormal F32 arithmetic to zero, so a kernel that
// must keep subnormals never holds a field value in a `float`.

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

// SoftFloat's f32_to_f64 of the F32 with bits `u`: exact.
static d64 d_from_f32_bits(uint u) {
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

static inline d64 d_from_f32(float x) {
    return d_from_f32_bits(as_type<uint>(x));
}

static inline uint shift_right_jam32(uint a, uint dist) {
    return (dist < 31u) ? ((a >> dist) | uint((a << (32u - dist)) != 0u)) : uint(a != 0u);
}

// SoftFloat's f64_to_f32 with softfloat_roundPackToF32, nearest-even: the
// result's bits.
static uint d_to_f32_bits(d64 a) {
    bool s = d_sign(a);
    int e = d_exp(a);
    ulong f = d_frac(a);
    uint sign = s ? 0x80000000u : 0u;
    if (e == 0x7FF) {
        return 0x7FC00000u;
    }
    uint sig = uint((f >> 22) | ulong((f & 0x3FFFFFul) != 0ul));
    if ((uint(e) | sig) == 0u) {
        return sign;
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
            return sign | 0x7F800000u;
        }
    }
    sig = (sig + 0x40u) >> 7;
    if (round_bits == 0x40u) {
        sig &= ~1u;
    }
    if (sig == 0u) {
        e = 0;
    }
    return sign + (uint(e) << 23) + sig;
}

static inline float d_to_f32(d64 a) {
    return as_type<float>(d_to_f32_bits(a));
}

// The exact binary64 of `v`.
static d64 d_from_u32(uint v) {
    if (v == 0u) {
        return 0ul;
    }
    uint shift = clz(v);
    return (ulong(1023u + 31u - shift) << 52) | ((ulong(v) << (21u + shift)) & D_FRAC);
}

static inline bool d_finite(d64 a) {
    return d_exp(a) != 0x7FF;
}

// `a < b` for operands that are not NaN; -0 and +0 are equal.
static bool d_lt(d64 a, d64 b) {
    if (((a | b) << 1) == 0ul) {
        return false;
    }
    bool sa = d_sign(a);
    if (sa != d_sign(b)) {
        return sa;
    }
    return sa ? (a > b) : (a < b);
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

// Nonnegative binary64 values order as their bit patterns, so the
// comparisons below compare bits.

// numerics::exp_series. Each divisor `f64::from(term)` is the exact
// conversion of the loop counter. With the divisors read from a
// program-scope `constant ulong` table instead, FL-031's weights kernel
// (Apple M5 Ultra, macOS 27) returned wrong exponentials for every cell,
// while the same function in a smaller kernel was right, and unrelated edits
// to the weights kernel changed the outcome. The cause is not established
// (a compiler fault is suspected). With the conversion every kernel tested
// gives the reference's values; tests/field.rs and tests/arithmetic.rs are
// the guards.
static d64 exp_series(d64 y, int terms) {
    d64 series = ONE;
    for (int term = terms; term >= 1; --term) {
        series = d_sub(ONE, d_mul(d_div(y, d_from_u32(uint(term))), series));
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
        series = d_sub(ONE, d_mul(d_div(y, d_from_u32(uint(term))), series));
    }
    d64 m = d_mul(y, series);
    for (int i = 0; i < halvings; ++i) {
        m = d_mul(m, d_sub(TWO, m));
    }
    return m;
}

// numerics::tanh for finite x.
static d64 d_tanh(d64 x) {
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
    return value;
}

// `numerics::tanh(f64::from(z)) as f32` for finite z.
static float tanh_reference(float z) {
    return d_to_f32(d_tanh(d_from_f32(z)));
}

// ---------------------------------------------------------------------------
// Phases 6 and 7's arithmetic (FL-032): flife-core's ledger helpers and the
// comparisons Rust makes, for operands that are not NaN.

// `f64::next_down`: the next value toward negative infinity. Both zeros go
// to the negative smallest subnormal; negative infinity stays.
static d64 d_next_down(d64 a) {
    if ((a << 1) == 0ul) {
        return D_SIGN | 1ul;
    }
    if (a == (D_SIGN | 0x7FF0000000000000ul)) {
        return a;
    }
    return d_sign(a) ? a + 1ul : a - 1ul;
}

// `a == b` as IEEE compares: the zeros are equal.
static inline bool d_eq(d64 a, d64 b) {
    return a == b || ((a | b) << 1) == 0ul;
}

// `|a| >= |b|`: nonnegative magnitudes order as their bit patterns.
static inline bool d_abs_ge(d64 a, d64 b) {
    return (a & ~D_SIGN) >= (b & ~D_SIGN);
}

// `f64::max` as it runs on the reference's host (AArch64 `fmaxnm`): the
// larger, and +0 over -0.
static d64 d_max(d64 a, d64 b) {
    if (d_lt(a, b)) {
        return b;
    }
    if (d_lt(b, a)) {
        return a;
    }
    return a & b;
}

// `f64::min` as it runs on the reference's host (AArch64 `fminnm`): the
// smaller, and -0 under +0.
static d64 d_min(d64 a, d64 b) {
    if (d_lt(a, b)) {
        return a;
    }
    if (d_lt(b, a)) {
        return b;
    }
    return a | b;
}

// ledger::add_down: `a + b`, rounded so that it never exceeds the exact sum
// (TwoSum's error, then one step down when the rounded sum is above).
static d64 d_add_down(d64 a, d64 b) {
    d64 sum = d_add(a, b);
    d64 b_part = d_sub(sum, a);
    d64 a_part = d_sub(sum, b_part);
    d64 error = d_add(d_sub(a, a_part), d_sub(b, b_part));
    return d_lt(error, 0ul) ? d_next_down(sum) : sum;
}

// ledger::sub_down.
static inline d64 d_sub_down(d64 a, d64 b) {
    return d_add_down(a, b ^ D_SIGN);
}

// ledger::CompensatedSum.
struct CSum {
    d64 sum;
    d64 compensation;
};

static inline CSum csum_zero() {
    CSum s;
    s.sum = 0ul;
    s.compensation = 0ul;
    return s;
}

static void csum_add(thread CSum &s, d64 value) {
    d64 total = d_add(s.sum, value);
    if (d_abs_ge(s.sum, value)) {
        s.compensation = d_add(s.compensation, d_add(d_sub(s.sum, total), value));
    } else {
        s.compensation = d_add(s.compensation, d_add(d_sub(value, total), s.sum));
    }
    s.sum = total;
}

static inline d64 csum_value(CSum s) {
    return d_add(s.sum, s.compensation);
}

// `f64::sqrt`, correctly rounded. With a = m 2^p, m an integer and p made
// even, q = floor(sqrt(m 2^54)) is found one bit per step (restoring), so q
// holds the 53 result bits and one rounding bit, and the remainder
// m 2^54 - q^2 is the sticky bit. Both zeros are their own roots; NaN below
// zero.
static d64 d_sqrt(d64 a) {
    int e = d_exp(a);
    ulong f = d_frac(a);
    if (e == 0x7FF) {
        return (f != 0ul || d_sign(a)) ? D_QNAN : a;
    }
    if ((a << 1) == 0ul) {
        return a;
    }
    if (d_sign(a)) {
        return D_QNAN;
    }
    if (e == 0) {
        norm_subnormal(e, f);
    }
    ulong m = f | D_HIDDEN;
    int power = e - 1075;
    if ((power & 1) != 0) {
        m <<= 1;
        power -= 1;
    }
    ulong q = 0ul;
    ulong r = 0ul;
    for (int j = 0; j < 54; ++j) {
        ulong pair = (j <= 26) ? ((m >> uint(52 - 2 * j)) & 3ul) : 0ul;
        r = (r << 2) | pair;
        ulong trial = (q << 2) | 1ul;
        q <<= 1;
        if (r >= trial) {
            r -= trial;
            q |= 1ul;
        }
    }
    ulong sig = q >> 1;
    if ((q & 1ul) != 0ul && (r != 0ul || (sig & 1ul) != 0ul)) {
        sig += 1ul;
    }
    // sig 2^(p/2 - 26); a carry into bit 53 carries into the exponent.
    return d_pack(false, power / 2 + 1048, sig);
}

// `a % b`, C's fmod, which is exact: musl's integer long division. The
// result has a's sign; NaN for a zero or NaN b or a non-finite a.
static d64 d_fmod(d64 a, d64 b) {
    int ea = d_exp(a);
    int eb = d_exp(b);
    ulong sign = a & D_SIGN;
    if ((b << 1) == 0ul || (eb == 0x7FF && d_frac(b) != 0ul) || ea == 0x7FF) {
        return D_QNAN;
    }
    if ((a << 1) <= (b << 1)) {
        return ((a << 1) == (b << 1)) ? sign : a;
    }
    ulong ma = d_frac(a);
    ulong mb = d_frac(b);
    if (ea == 0) {
        norm_subnormal(ea, ma);
    } else {
        ma |= D_HIDDEN;
    }
    if (eb == 0) {
        norm_subnormal(eb, mb);
    } else {
        mb |= D_HIDDEN;
    }
    for (; ea > eb; --ea) {
        ulong diff = ma - mb;
        if ((diff >> 63) == 0ul) {
            if (diff == 0ul) {
                return sign;
            }
            ma = diff;
        }
        ma <<= 1;
    }
    ulong diff = ma - mb;
    if ((diff >> 63) == 0ul) {
        if (diff == 0ul) {
            return sign;
        }
        ma = diff;
    }
    for (; (ma >> 52) == 0ul; ma <<= 1) {
        --ea;
    }
    if (ea > 0) {
        ma = (ma - D_HIDDEN) | (ulong(uint(ea)) << 52);
    } else {
        ma >>= uint(1 - ea);
    }
    return ma | sign;
}

// `f64::rem_euclid(a, b)`: `a % b`, plus |b| when that is below zero.
static d64 d_rem_euclid(d64 a, d64 b) {
    d64 r = d_fmod(a, b);
    return d_lt(r, 0ul) ? d_add(r, b & ~D_SIGN) : r;
}

// `f64::floor`: integral values (and the zeros) are their own floors; a
// negative fraction rounds its magnitude up, carrying into the exponent.
static d64 d_floor(d64 a) {
    int e = d_exp(a) - 1023;
    if (e >= 52) {
        return a;
    }
    if (e < 0) {
        if ((a << 1) == 0ul) {
            return a;
        }
        return d_sign(a) ? (D_SIGN | ONE) : 0ul;
    }
    ulong mask = D_FRAC >> uint(e);
    if ((a & mask) == 0ul) {
        return a;
    }
    return d_sign(a) ? ((a & ~mask) + mask + 1ul) : (a & ~mask);
}

// `as i64` of an integral value below 2^62 in magnitude.
static long d_to_long(d64 a) {
    int e = d_exp(a) - 1023;
    if (e < 0) {
        return 0;
    }
    ulong m = d_frac(a) | D_HIDDEN;
    long v = long(e >= 52 ? (m << uint(e - 52)) : (m >> uint(52 - e)));
    return d_sign(a) ? -v : v;
}

// flife-field's wrap_coordinate for a finite coordinate and a finite
// positive length: the Euclidean remainder, and +0 for one that rounds up
// to the length or is zero.
static d64 d_wrap(d64 x, d64 length) {
    d64 wrapped = d_rem_euclid(x, length);
    if (!d_lt(wrapped, length) || d_eq(wrapped, 0ul)) {
        return 0ul;
    }
    return wrapped;
}
