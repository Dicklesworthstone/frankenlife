// FL-032: phase 7's reproduction proposal on the device, one thread per
// organism alive after emission, after softfloat64.metal and philox.metal:
// flife-engine's Reproduction::propose, operation for operation on software
// binary64. The host builds the birth intent from the split.
//
// As in chemistry.metal, a thread that meets anything the reference would
// refuse writes a nonzero status and the host refuses the call.

// params: [0] organisms; then word pairs: [1] the world seed, [3] the birth
// stream's code, [5] the attempt lane's word, [7] the minimum attempt
// energy, [9] the birth cost, [11] the minimum child energy and [13] the
// minimum parent energy.
constant uint P_PARAMS = 15u;

// An organism: word pairs [0] its identity, [2] the attempt tick, [4] its
// cooldown deadline, [6] its post-emission balance, [8] its attempt
// probability and [10] its child fraction; [12] whether its budget is
// exhausted, a word.
constant uint P_ORGANISM = 13u;

// An outcome: [0] status; [1] the proposal; [2] why an attempt is not
// viable; word pairs [3] the energy available, [5] the child's share and [7]
// the parent's.
constant uint P_OUTCOME = 9u;

constant uint PROPOSAL_EXHAUSTED = 0u;
constant uint PROPOSAL_COOLING_DOWN = 1u;
constant uint PROPOSAL_NO_ATTEMPT = 2u;
constant uint PROPOSAL_UNVIABLE = 3u;
constant uint PROPOSAL_VIABLE = 4u;

constant uint UNVIABLE_ATTEMPT_ENERGY = 0u;
constant uint UNVIABLE_BIRTH_COST = 1u;
constant uint UNVIABLE_CHILD = 2u;
constant uint UNVIABLE_PARENT = 3u;

constant uint STATUS_PROBABILITY = 1u;  // an attempt probability outside [0, 1]
constant uint STATUS_ENERGY = 2u;       // a balance that is not finite and nonnegative
constant uint STATUS_CHILD = 3u;        // a child fraction outside (0, 1)

static inline ulong p_load(device const uint *words, uint at) {
    return ulong(words[at]) | (ulong(words[at + 1u]) << 32);
}

static inline void p_store(device uint *words, uint at, ulong value) {
    words[at] = uint(value & 0xFFFFFFFFul);
    words[at + 1u] = uint(value >> 32);
}

kernel void proposal_settle(
    device const uint *params [[buffer(0)]],
    device const uint *organisms [[buffer(1)]],
    device uint *out [[buffer(2)]],
    uint index [[thread_position_in_grid]])
{
    if (index >= params[0]) {
        return;
    }
    device const uint *organism = organisms + index * P_ORGANISM;
    device uint *outcome = out + index * P_OUTCOME;
    for (uint k = 0u; k < P_OUTCOME; ++k) {
        outcome[k] = 0u;
    }
    if (organism[12] != 0u) {
        outcome[1] = PROPOSAL_EXHAUSTED;
        return;
    }
    ulong tick = p_load(organism, 2u);
    if (tick < p_load(organism, 4u)) {
        outcome[1] = PROPOSAL_COOLING_DOWN;
        return;
    }

    // Reproduction::attempts: word 0 of (birth, parent, tick, lane
    // (attempt, 0), 0) against the probability.
    d64 probability = p_load(organism, 8u);
    if (!d_finite(probability) || d_lt(probability, 0ul) || d_lt(ONE, probability)) {
        outcome[0] = STATUS_PROBABILITY;
        return;
    }
    PhiloxBlock block = philox_block(p_load(params, 1u), p_load(params, 3u),
                                     p_load(organism, 0u), tick, p_load(params, 5u), 0ul);
    if (!d_lt(d_half_open_unit(block.words[0]), probability)) {
        outcome[1] = PROPOSAL_NO_ATTEMPT;
        return;
    }

    // Reproduction::split of the post-emission balance.
    d64 available = p_load(organism, 6u);
    if (!d_finite(available) || d_lt(available, 0ul)) {
        outcome[0] = STATUS_ENERGY;
        return;
    }
    d64 fraction = p_load(organism, 10u);
    if (!d_finite(fraction) || !d_lt(0ul, fraction) || !d_lt(fraction, ONE)) {
        outcome[0] = STATUS_CHILD;
        return;
    }
    outcome[1] = PROPOSAL_UNVIABLE;
    if (d_lt(available, p_load(params, 7u))) {
        outcome[2] = UNVIABLE_ATTEMPT_ENERGY;
        p_store(outcome, 3u, available);
        return;
    }
    d64 remainder = d_sub_down(available, p_load(params, 9u));
    if (!d_lt(0ul, remainder)) {
        outcome[2] = UNVIABLE_BIRTH_COST;
        p_store(outcome, 3u, available);
        return;
    }
    d64 child = d_mul(fraction, remainder);
    d64 parent = d_sub_down(remainder, child);
    if (d_lt(child, p_load(params, 11u))) {
        outcome[2] = UNVIABLE_CHILD;
        p_store(outcome, 5u, child);
        return;
    }
    if (d_lt(parent, p_load(params, 13u))) {
        outcome[2] = UNVIABLE_PARENT;
        p_store(outcome, 7u, parent);
        return;
    }
    outcome[1] = PROPOSAL_VIABLE;
    p_store(outcome, 3u, d_add(available, 0ul));
    p_store(outcome, 5u, child);
    p_store(outcome, 7u, parent);
}

// The randomness alone, for the tests: per row of seven word pairs (the
// key's seed and stream, the counter's entity, ordinal, lane and draw, and
// one more word), a row of word pairs: the block's four words, each one's
// half-open uniform, and the extra word's.
constant uint P_ADDRESS = 14u;
constant uint P_OPS = 18u;

kernel void proposal_ops(
    device const uint *addresses [[buffer(0)]],
    device uint *out [[buffer(1)]],
    uint index [[thread_position_in_grid]])
{
    device const uint *address = addresses + index * P_ADDRESS;
    PhiloxBlock block = philox_block(p_load(address, 0u), p_load(address, 2u), p_load(address, 4u),
                                     p_load(address, 6u), p_load(address, 8u), p_load(address, 10u));
    device uint *row = out + index * P_OPS;
    for (uint k = 0u; k < 4u; ++k) {
        p_store(row, 2u * k, block.words[k]);
        p_store(row, 8u + 2u * k, d_half_open_unit(block.words[k]));
    }
    p_store(row, 16u, d_half_open_unit(p_load(address, 12u)));
}
