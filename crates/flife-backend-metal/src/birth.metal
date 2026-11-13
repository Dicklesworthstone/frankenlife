// FL-032: phase 8's parameter mutation on the device, one thread per
// parameter of every child in the call, after softfloat64.metal,
// philox.metal and numerics.metal: flife-genome's Mutator::mutate_parameters
// on the child's parameters (the parent's, or the parent's mapped onto the
// candidate's member), operation for operation on software binary64.
//
// For the parameter at (layer, output, input), or the bias at (layer,
// output), lane (candidate, weight or bias, neuron) draw 0 decides: word 0
// passing the replacement probability replaces it with the fan-in draw of
// word 1; otherwise word 2 passing the weight probability perturbs it by the
// deviation times component 0 of draw 1's Box-Muller pair, in binary64,
// rounded once to the dtype.

// params: [0] parameters in the call; [1] births; [2] dtype (0 F32, 1 F16);
// then word pairs: [3] the world seed, [5] the mutation stream's code, [7]
// the replacement probability, [9] the weight probability and [11] the
// weight deviation.
constant uint M_PARAMS = 13u;

// A birth: word pairs [0] the parent and [2] the attempt's tick; [4] the
// candidate; [5] its first parameter's index in the call; [6] its parameter
// count; [7] its layers; then per layer, four words: fan-in, outputs, the
// first weight's offset and the first bias's.
constant uint M_BIRTH = 20u;
constant uint M_LAYERS = 3u;

constant uint KEPT = 0u;
constant uint REPLACED = 1u;
constant uint PERTURBED = 2u;

constant ulong LANE_WEIGHT = 2ul;
constant ulong LANE_BIAS = 3ul;

static inline ulong m_load(device const uint *words, uint at) {
    return ulong(words[at]) | (ulong(words[at + 1u]) << 32);
}

static inline void m_store(device uint *words, uint at, ulong value) {
    words[at] = uint(value & 0xFFFFFFFFul);
    words[at + 1u] = uint(value >> 32);
}

// A parameter's value in binary64, exactly.
static d64 m_widen(uint bits, uint dtype) {
    return d_from_f32_bits(dtype == 0u ? bits : f16_to_f32_bits(bits));
}

// Weight::round_from_f64: to nearest, ties to even, in the dtype.
static uint m_round(d64 value, uint dtype) {
    return dtype == 0u ? d_to_f32_bits(value) : d_to_f16_bits(value);
}

// init::fan_in_draw before its rounding: `(2u - 1) sqrt(3 / fan_in)`.
static d64 fan_in_value(ulong word, uint fan_in) {
    d64 limit = d_sqrt(d_div(THREE, d_from_u32(fan_in)));
    return d_mul(d_sub(d_mul(TWO, d_half_open_unit(word)), ONE), limit);
}

kernel void birth_parameters(
    device const uint *params [[buffer(0)]],
    device const uint *births [[buffer(1)]],
    device uint *values [[buffer(2)]],
    device uint *codes [[buffer(3)]],
    uint index [[thread_position_in_grid]])
{
    if (index >= params[0]) {
        return;
    }
    // The last birth whose first parameter is at or before this one.
    uint low = 0u;
    uint high = params[1];
    while (high - low > 1u) {
        uint middle = low + (high - low) / 2u;
        if (births[middle * M_BIRTH + 5u] <= index) {
            low = middle;
        } else {
            high = middle;
        }
    }
    device const uint *birth = births + low * M_BIRTH;
    uint at = index - birth[5];
    codes[index] = KEPT;

    // The parameter's layer, neuron and kind; one outside every layer is
    // left as it is.
    ulong neuron = 0ul;
    ulong kind = 0ul;
    uint fan_in = 0u;
    for (uint layer = 0u; layer < birth[7] && layer < M_LAYERS; ++layer) {
        device const uint *shape = birth + 8u + 4u * layer;
        uint inputs = shape[0];
        uint outputs = shape[1];
        uint weights = shape[2];
        uint biases = shape[3];
        if (at >= weights && at < weights + outputs * inputs) {
            uint offset = at - weights;
            neuron = (ulong(layer) << 32) | (ulong(offset / inputs) << 16) | ulong(offset % inputs);
            kind = LANE_WEIGHT;
            fan_in = inputs;
            break;
        }
        if (at >= biases && at < biases + outputs) {
            neuron = (ulong(layer) << 32) | (ulong(at - biases) << 16);
            kind = LANE_BIAS;
            fan_in = inputs;
            break;
        }
    }
    if (kind == 0ul) {
        return;
    }
    uint dtype = params[2];
    ulong lane = (ulong(birth[4]) << 56) | (kind << 48) | neuron;
    ulong seed = m_load(params, 3u);
    ulong stream = m_load(params, 5u);
    ulong parent = m_load(birth, 0u);
    ulong tick = m_load(birth, 2u);
    PhiloxBlock decide = philox_block(seed, stream, parent, tick, lane, 0ul);
    if (d_lt(d_half_open_unit(decide.words[0]), m_load(params, 7u))) {
        values[index] = m_round(fan_in_value(decide.words[1], fan_in), dtype);
        codes[index] = REPLACED;
    } else if (d_lt(d_half_open_unit(decide.words[2]), m_load(params, 9u))) {
        PhiloxBlock draw = philox_block(seed, stream, parent, tick, lane, 1ul);
        d64 normal;
        d64 unused;
        box_muller(draw.words[0], draw.words[1], normal, unused);
        d64 delta = d_mul(m_load(params, 11u), normal);
        values[index] = m_round(d_add(m_widen(values[index], dtype), delta), dtype);
        codes[index] = PERTURBED;
    }
}

// The arithmetic alone, for the tests: per row of eight word pairs (x for
// exp, y for ln, u for sin_cos_turns, words w1 and w2, f to round to
// binary16, binary16 bits h and a fan-in), a row of twelve word pairs:
// whether exp(x) is defined, exp(x), ln(y), sin and cos of turn u,
// open_unit(w1), Box-Muller's two components of (w1, w2), the binary16 bits
// of f, the binary32 bits of h, the fan-in draw of w1 before rounding, and
// that draw rounded to binary32 (low word) and binary16 (high word).
constant uint M_OPS_IN = 16u;
constant uint M_OPS_OUT = 24u;

kernel void birth_ops(
    device const uint *rows [[buffer(0)]],
    device uint *out [[buffer(1)]],
    uint index [[thread_position_in_grid]])
{
    device const uint *row = rows + index * M_OPS_IN;
    device uint *result = out + index * M_OPS_OUT;
    d64 exp_value = 0ul;
    bool defined = exp_signed(m_load(row, 0u), exp_value);
    m_store(result, 0u, defined ? 1ul : 0ul);
    m_store(result, 2u, exp_value);
    m_store(result, 4u, d_ln(m_load(row, 2u)));
    d64 s;
    d64 c;
    sin_cos_turns(m_load(row, 4u), s, c);
    m_store(result, 6u, s);
    m_store(result, 8u, c);
    ulong first = m_load(row, 6u);
    m_store(result, 10u, d_open_unit(first));
    d64 zero;
    d64 one;
    box_muller(first, m_load(row, 8u), zero, one);
    m_store(result, 12u, zero);
    m_store(result, 14u, one);
    m_store(result, 16u, ulong(d_to_f16_bits(m_load(row, 10u))));
    m_store(result, 18u, ulong(f16_to_f32_bits(row[12])));
    d64 drawn = fan_in_value(first, row[14]);
    m_store(result, 20u, drawn);
    result[22] = d_to_f32_bits(drawn);
    result[23] = d_to_f16_bits(drawn);
}
