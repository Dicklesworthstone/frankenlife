// FL-032, first stage: phase 6's chemistry on the device, one thread per
// organism, after softfloat64.metal. flife-engine's `composition`,
// `Chemistry::efficiency`, `settle_efficiencies` and `EnergyBudget::credit`
// and `pay`, operation for operation on software binary64.
//
// A thread that meets anything the reference would refuse, or a value it
// cannot reproduce (an infinity, which softfloat64 turns into a NaN), writes
// a nonzero status and nothing else the host may use: the host refuses the
// call and the reference decides the tick.

// The field: 33 planes of F32 bits, energy first, then the 32 moments.
// The F32 values are read as bits, never as `float`, so subnormals stay.

// params: [0] cells, [1] organisms, then the run's constants as word pairs:
// [2] eta_max, [4] gain k, [6] threshold b, [8] toxicity sensitivity kappa,
// [10] the affinity norm limit.
constant uint C_PARAMS = 12u;

// An organism: [0..64] its affinity, 32 word pairs; [64] its budget's
// remaining energy; [66] whether the budget is exhausted; [67] the index of
// its first packet; [68] its packet count.
constant uint C_ORGANISM = 69u;

// A packet: [0] its cell, [1] the granted energy as a word pair.
constant uint C_PACKET = 3u;

// An outcome: [0] status; then word pairs [1] absorbed U, [3] gain G,
// [5] requested toxicity, [7] paid toxicity, [9] the remaining energy; and
// [11] whether the budget is exhausted.
constant uint C_OUTCOME = 12u;

constant uint C_CHANNELS = 32u;

constant uint STATUS_COMPOSITION = 1u;   // a non-finite a.z (tanh's argument)
constant uint STATUS_GRANTED = 2u;       // a negative or non-finite grant
constant uint STATUS_EFFICIENCY = 3u;    // an efficiency outside [-1, 1]
constant uint STATUS_EXHAUSTED = 4u;     // a gain credited to an exhausted budget
constant uint STATUS_CREDIT = 5u;        // a credit that does not stay finite
constant uint STATUS_TOXICITY = 6u;      // a non-finite or negative toxicity request
constant uint STATUS_CELL = 7u;          // a cell outside the field
constant uint STATUS_SUM = 8u;           // a non-finite total
constant uint STATUS_AFFINITY = 9u;      // a non-finite affinity or one past the limit

constant ulong NEG_ONE = 0xBFF0000000000000ul;

static inline d64 c_load(device const uint *words, uint at) {
    return ulong(words[at]) | (ulong(words[at + 1u]) << 32);
}

static inline void c_store(device uint *words, uint at, d64 value) {
    words[at] = uint(value & 0xFFFFFFFFul);
    words[at + 1u] = uint(value >> 32);
}

kernel void chemistry_settle(
    device const uint *field [[buffer(0)]],
    device const uint *params [[buffer(1)]],
    device const uint *organisms [[buffer(2)]],
    device const uint *packets [[buffer(3)]],
    device uint *out [[buffer(4)]],
    uint index [[thread_position_in_grid]])
{
    uint cells = params[0];
    if (index >= params[1]) {
        return;
    }
    d64 eta = c_load(params, 2u);
    d64 gain_k = c_load(params, 4u);
    d64 threshold = c_load(params, 6u);
    d64 kappa = c_load(params, 8u);
    device const uint *organism = organisms + index * C_ORGANISM;
    device uint *outcome = out + index * C_OUTCOME;
    d64 remaining = c_load(organism, 64u);
    bool exhausted = organism[66] != 0u;
    uint first = organism[67];
    uint count = organism[68];

    // Affinity::new: finite components whose squares, summed in order, are
    // at most the limit's square.
    d64 squared = 0ul;
    for (uint c = 0u; c < C_CHANNELS; ++c) {
        d64 component = c_load(organism, 2u * c);
        if (!d_finite(component)) {
            outcome[0] = STATUS_AFFINITY;
            return;
        }
        squared = d_add(squared, d_mul(component, component));
    }
    d64 limit = c_load(params, 10u);
    d64 bound = d_mul(limit, limit);
    if (!d_finite(squared) || d_lt(bound, squared)) {
        outcome[0] = STATUS_AFFINITY;
        return;
    }

    // Chemistry::settle: each packet's efficiency, then settle_efficiencies'
    // sums in packet order.
    CSum absorbed = csum_zero();
    CSum gain = csum_zero();
    CSum harm = csum_zero();
    for (uint p = 0u; p < count; ++p) {
        device const uint *packet = packets + (first + p) * C_PACKET;
        uint cell = packet[0];
        d64 granted = c_load(packet, 1u);
        if (cell >= cells) {
            outcome[0] = STATUS_CELL;
            return;
        }
        // composition: z = m / e in F64 from the exact F32 values, the
        // neutral z = 0 where e is not positive; then a.z in channel order.
        d64 energy = d_from_f32_bits(field[cell]);
        d64 dot = 0ul;
        if (d_lt(0ul, energy)) {
            for (uint c = 0u; c < C_CHANNELS; ++c) {
                d64 moment = d_from_f32_bits(field[(c + 1u) * cells + cell]);
                d64 z = d_div(moment, energy);
                dot = d_add(dot, d_mul(c_load(organism, 2u * c), z));
            }
        } else {
            for (uint c = 0u; c < C_CHANNELS; ++c) {
                dot = d_add(dot, d_mul(c_load(organism, 2u * c), 0ul));
            }
        }
        // Chemistry::efficiency: eta_max tanh(k a.z - b).
        d64 argument = d_sub(d_mul(gain_k, dot), threshold);
        if (!d_finite(argument)) {
            outcome[0] = STATUS_COMPOSITION;
            return;
        }
        d64 efficiency = d_mul(eta, d_tanh(argument));
        // settle_efficiencies' checks.
        if (!d_finite(granted) || d_lt(granted, 0ul)) {
            outcome[0] = STATUS_GRANTED;
            return;
        }
        if (!d_finite(efficiency) || d_lt(efficiency, NEG_ONE) || d_lt(ONE, efficiency)) {
            outcome[0] = STATUS_EFFICIENCY;
            return;
        }
        csum_add(absorbed, granted);
        csum_add(gain, d_mul(d_max(efficiency, 0ul), granted));
        csum_add(harm, d_mul(d_max(efficiency ^ D_SIGN, 0ul), granted));
    }
    d64 total = csum_value(absorbed);
    d64 credited = csum_value(gain);
    d64 requested = d_mul(kappa, csum_value(harm));
    if (!d_finite(total) || !d_finite(credited) || !d_finite(requested)) {
        outcome[0] = STATUS_SUM;
        return;
    }

    // EnergyBudget::credit of G when it is positive.
    if (d_lt(0ul, credited)) {
        if (exhausted) {
            outcome[0] = STATUS_EXHAUSTED;
            return;
        }
        remaining = d_add_down(remaining, d_add(credited, 0ul));
        if (!d_finite(remaining)) {
            outcome[0] = STATUS_CREDIT;
            return;
        }
    }

    // EnergyBudget::pay of the requested toxicity.
    if (d_lt(requested, 0ul)) {
        outcome[0] = STATUS_TOXICITY;
        return;
    }
    requested = d_add(requested, 0ul);
    d64 paid = d_min(requested, remaining);
    remaining = d_eq(paid, remaining) ? 0ul : d_sub_down(remaining, paid);
    if (d_eq(remaining, 0ul) && d_lt(0ul, paid)) {
        exhausted = true;
    }

    outcome[0] = 0u;
    c_store(outcome, 1u, total);
    c_store(outcome, 3u, credited);
    c_store(outcome, 5u, requested);
    c_store(outcome, 7u, paid);
    c_store(outcome, 9u, remaining);
    outcome[11] = exhausted ? 1u : 0u;
}

// The arithmetic alone, for the tests: per operand pair (four words), a row
// of word pairs: tanh(a), add_down(a, b), sub_down(a, b), next_down(a),
// max(a, b), min(a, b), and a compensated sum of a, b, -a, b.
constant uint C_OPS = 14u;

kernel void settle_ops(
    device const uint *operands [[buffer(0)]],
    device uint *out [[buffer(1)]],
    uint index [[thread_position_in_grid]])
{
    d64 a = c_load(operands, 4u * index);
    d64 b = c_load(operands, 4u * index + 2u);
    CSum s = csum_zero();
    csum_add(s, a);
    csum_add(s, b);
    csum_add(s, a ^ D_SIGN);
    csum_add(s, b);
    d64 results[7] = {
        d_finite(a) ? d_tanh(a) : D_QNAN,
        d_add_down(a, b),
        d_sub_down(a, b),
        d_next_down(a),
        d_max(a, b),
        d_min(a, b),
        csum_value(s),
    };
    device uint *row = out + index * C_OPS;
    for (uint k = 0u; k < 7u; ++k) {
        c_store(row, 2u * k, results[k]);
    }
}
