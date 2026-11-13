// FrankenLife's fused controller kernel (plan §10.2, FL-030): every due
// organism's own dense tanh network, one threadgroup per organism, in the
// reference controller's declared arithmetic, bit for bit.
//
// The reference (flife-neural's ReferenceController) accumulates each neuron
// in F32, bias first and then the inputs in ascending index, rounding every
// product and every sum, with no fused multiply-add. Its hidden activation is
// flife-core's `numerics::tanh`, evaluated in F64 and rounded once to F32.
// Apple GPUs have no F64, so the activation runs softfloat64.metal's
// software IEEE-754 binary64 (compiled ahead of this file): the reference
// algorithm operation for operation, so every intermediate is the CPU's
// value and the rounded result is the CPU's.

// ---------------------------------------------------------------------------
// The fast path: the same activation from hardware F32 arithmetic in
// float-float (an unevaluated sum hi + lo, about 48 bits), accepted only
// where it provably rounds as the reference does.
//
// The reference's F64 value T is within 2^-51 of tanh, and the float-float
// value V within about 2^-44. Every value within 2^-38 |V| of V rounds to
// the same F32 unless an F32 rounding midpoint lies that close; then the
// lane falls back to softfloat64.metal's binary64. The exhaustive test checks
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
// holds five words per organism: its row (`slot`) in `inputs`, `outputs` and
// `faults`, the offset of its parameters in `params` (in elements of `W`),
// its hidden-layer count (1 or 2) and its hidden widths. One dispatch serves
// one parameter shard, so the rows of a command's dispatches interleave. A
// nonzero word in `faults` defers the organism to the host, which evaluates
// it on the reference controller: for its error, or for its exact value.
//
// `W` is the shard's weight type: `float`, or `half` for compact F16 genomes
// (FL-030). Every parameter is read as `float(p)`, which is exact for both,
// and the arithmetic after it is the same F32 arithmetic.
template <typename W>
static void forward_pass(
    device const W *params,
    device const float *inputs,
    device const uint *jobs,
    device float *outputs,
    device uint *faults,
    uint organism,
    uint lane,
    uint lanes,
    threadgroup float *sensed,
    threadgroup float *first,
    threadgroup float *second,
    threadgroup atomic_uint *fault)
{
    uint fallbacks = 0u;
    if (lane == 0u) {
        atomic_store_explicit(fault, 0u, memory_order_relaxed);
    }
    uint slot = jobs[organism * 5u];
    for (uint i = lane; i < SENSORS; i += lanes) {
        sensed[i] = inputs[slot * SENSORS + i];
    }
    threadgroup_barrier(mem_flags::mem_threadgroup);

    uint offset = jobs[organism * 5u + 1u];
    uint hidden = jobs[organism * 5u + 2u];
    uint widths[2] = {jobs[organism * 5u + 3u], jobs[organism * 5u + 4u]};
    threadgroup const float *source = sensed;
    uint fan_in = SENSORS;
    for (uint layer = 0u; layer <= hidden; ++layer) {
        bool last = layer == hidden;
        uint fan_out = last ? ACTIONS : widths[layer];
        threadgroup float *target = (layer == 0u) ? first : second;
        device const W *weights = params + offset;
        device const W *biases = weights + fan_in * fan_out;
        for (uint o = lane; o < fan_out; o += lanes) {
            device const W *row = weights + o * fan_in;
            float z = float(biases[o]);
            bool tiny = false;
            for (uint i = 0u; i < fan_in; ++i) {
                float w = float(row[i]);
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
                atomic_fetch_or_explicit(fault, FAULT_TINY, memory_order_relaxed);
            }
            if (!isfinite(z)) {
                atomic_fetch_or_explicit(fault, FAULT_NONFINITE, memory_order_relaxed);
            }
            if (last) {
                outputs[slot * ACTIONS + o] = z;
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
        faults[slot] = atomic_load_explicit(fault, memory_order_relaxed);
    }
}

// The F32 shards' forward pass.
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
    forward_pass<float>(params, inputs, jobs, outputs, faults, organism, lane, lanes,
                        sensed, first, second, &fault);
}

// The binary16 shards' forward pass (compact F16, FL-030): half the bytes a
// parameter, the same outputs as the reference.
kernel void controller_forward_half(
    device const half *params [[buffer(0)]],
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
    forward_pass<half>(params, inputs, jobs, outputs, faults, organism, lane, lanes,
                       sensed, first, second, &fault);
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
