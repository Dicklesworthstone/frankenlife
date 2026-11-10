// FrankenLife's uptake kernels (plan §5.2, §8.1 phase 5, FL-031): each
// organism's footprint density and saturating request, and each cell's
// demand and grants under cell-quantum-largest-remainder-v1, in
// flife-field's exchange.rs and uptake.rs arithmetic, bit for bit, on
// softfloat64.metal's binary64.
//
// The F32 energy plane is read as bit patterns, so subnormal energies keep
// their value. Binary64 values are two words, low first. softfloat64 turns
// every non-finite operand into a quiet NaN where IEEE would give an
// infinity or zero, so the host refuses any non-finite density, request or
// demand these kernels return, and the tick is computed on the reference.

// An organism's footprint, in words: its cell count, four cells, then four
// binary64 weights.
constant uint STENCIL = 13u;
// The request parameters: organisms, a pad word, then the cell area,
// seconds per tick, capacity per second and half-saturation density (two
// words each).
// A cell, in words: its first packet, its packet count and its F32 energy's
// bits.
constant uint CELL = 3u;
// A resolved cell, in words: its demand A_c (two words), its target K_c,
// how far the floors miss it, and its mode.
constant uint RESOLVED = 5u;
constant uint MODE_NONE = 0u;
constant uint MODE_DEFICIT = 1u;
constant uint MODE_SURPLUS = 2u;

// 2^52: at and above it every binary64 is an integer.
constant d64 INTEGRAL = 0x4330000000000000ul;

static inline d64 u_word(device const uint *words, uint at) {
    return (ulong(words[at + 1u]) << 32) | ulong(words[at]);
}

static inline void u_store(device uint *words, uint at, d64 value) {
    words[at] = uint(value & 0xFFFFFFFFul);
    words[at + 1u] = uint(value >> 32);
}

static inline d64 d_abs(d64 a) {
    return a & ~D_SIGN;
}

// `f64::floor` of a finite value that is not negative: the fraction's bits
// cleared.
static d64 d_floor_nonneg(d64 a) {
    int e = d_exp(a) - 1023;
    if (e >= 52) {
        return a;
    }
    if (e < 0) {
        return a & D_SIGN;
    }
    return a & ~(D_FRAC >> uint(e));
}

// `f64::round_ties_even` of a finite value that is not negative: below
// 2^52, adding 2^52 rounds to the nearest integer with ties to even, and
// subtracting it again is exact.
static d64 d_round_even_nonneg(d64 a) {
    if (!d_lt(a, INTEGRAL)) {
        return a;
    }
    return d_sub(d_add(a, INTEGRAL), INTEGRAL);
}

// `as u64` of an integral value below 2^32 that is not negative.
static uint d_to_uint(d64 a) {
    int e = d_exp(a) - 1023;
    if (e < 0) {
        return 0u;
    }
    return uint((d_frac(a) | D_HIDDEN) >> uint(52 - e));
}

// `ExchangeStencil::density` and `saturating_request`: one thread per
// organism; its density and request, two words each.
kernel void uptake_request(
    device const uint *energy [[buffer(0)]],
    device const uint *stencils [[buffer(1)]],
    device const uint *params [[buffer(2)]],
    device uint *out [[buffer(3)]],
    uint id [[thread_position_in_grid]])
{
    if (id >= params[0]) {
        return;
    }
    device const uint *stencil = stencils + id * STENCIL;
    uint len = min(stencil[0], 4u);
    d64 amount = 0ul;
    for (uint k = 0u; k < len; k++) {
        d64 weight = u_word(stencil, 5u + 2u * k);
        amount = d_add(amount, d_mul(weight, d_from_f32_bits(energy[stencil[1u + k]])));
    }
    d64 density = d_div(amount, u_word(params, 2u));
    d64 seconds = u_word(params, 4u), capacity = u_word(params, 6u);
    d64 saturation = u_word(params, 8u);
    d64 request = d_div(d_mul(d_mul(seconds, capacity), density), d_add(saturation, density));
    u_store(out, 4u * id, density);
    u_store(out, 4u * id + 2u, request);
}

// One cell: its demand, the CompensatedSum of its packets' requests in
// identity order; its target (`cell_target`); and each packet's floor share
// and remainder. A deficit's packets are ranked next.
// params: cells, packets.
kernel void uptake_cells(
    device const uint *cells [[buffer(0)]],
    device const uint *requested [[buffer(1)]],
    device const uint *params [[buffer(2)]],
    device uint *resolved [[buffer(3)]],
    device uint *cell_of [[buffer(4)]],
    device uint *remainders [[buffer(5)]],
    device uint *quanta [[buffer(6)]],
    uint id [[thread_position_in_grid]])
{
    if (id >= params[0]) {
        return;
    }
    uint start = cells[CELL * id], len = cells[CELL * id + 1u], bits = cells[CELL * id + 2u];

    d64 sum = 0ul, compensation = 0ul;
    for (uint k = 0u; k < len; k++) {
        d64 value = u_word(requested, 2u * (start + k));
        d64 total = d_add(sum, value);
        if (!d_lt(d_abs(sum), d_abs(value))) {
            compensation = d_add(compensation, d_add(d_sub(sum, total), value));
        } else {
            compensation = d_add(compensation, d_add(d_sub(value, total), sum));
        }
        sum = total;
    }
    d64 demanded = d_add(sum, compensation);

    // `quanta_of`: the energy is n quanta of Q = 2^(max(exponent, 1) - 150).
    uint exponent = (bits >> 23) & 0xFFu;
    uint whole = (exponent == 0u) ? (bits & 0x7FFFFFu) : ((bits & 0x7FFFFFu) | 0x800000u);
    d64 quantum = ulong(max(exponent, 1u) + 873u) << 52;
    bool contended = d_lt(d_from_f32_bits(bits), demanded);

    uint target = 0u;
    ulong floors = 0ul;
    if (whole != 0u) {
        d64 scale;
        if (contended) {
            target = whole;
            scale = d_div(d_from_u32(whole), demanded);
        } else {
            d64 ideal = d_round_even_nonneg(d_div(demanded, quantum));
            d64 limit = d_from_u32(whole);
            target = d_to_uint(d_lt(limit, ideal) ? limit : ideal);
            scale = d_div(ONE, quantum);
        }
        for (uint k = 0u; k < len; k++) {
            uint at = start + k;
            d64 ideal = d_mul(u_word(requested, 2u * at), scale);
            d64 down = d_floor_nonneg(ideal);
            uint share = d_to_uint(down);
            floors += ulong(share);
            quanta[at] = share;
            u_store(remainders, 2u * at, d_sub(ideal, down));
            cell_of[at] = id;
        }
    } else {
        for (uint k = 0u; k < len; k++) {
            quanta[start + k] = 0u;
            cell_of[start + k] = id;
        }
    }

    uint mode = MODE_NONE, excess = 0u;
    if (floors < ulong(target)) {
        mode = MODE_DEFICIT;
        excess = target - uint(floors);
    } else if (floors > ulong(target)) {
        mode = MODE_SURPLUS;
        excess = uint(floors - ulong(target));
    }
    u_store(resolved, RESOLVED * id, demanded);
    resolved[RESOLVED * id + 2u] = target;
    resolved[RESOLVED * id + 3u] = excess;
    resolved[RESOLVED * id + 4u] = mode;
}

// One packet of a cell whose floors fall short of its target: its rank in
// the order `resolve_cell` sorts a deficit's packets in, largest remainder
// first and ties to the lower identity. A remainder is the difference of a
// nonnegative value and its floor, so it is +0 or positive and its bits order
// as `total_cmp` does. The reference's passes give every packet
// deficit / len more quanta and the first deficit % len in that order one
// more, which this applies.
//
// A cell whose floors exceed its target keeps them: the host's sum check
// then refuses it and the reference resolves the tick. No valid input makes
// one. Uncontended, the floors of q_i/Q sum to at most (sum of q_i)/Q, which
// exceeds A_c/Q, A_c being the compensated sum, by far less than the half
// quantum they would need to pass round(A_c/Q). Contended, the ideal shares
// sum to at most n_c (1 + 5 * 2^-53) < n_c + 1, so their floors are at most n_c.
// params: cells, packets.
kernel void uptake_rank(
    device const uint *cells [[buffer(0)]],
    device const uint *params [[buffer(1)]],
    device const uint *resolved [[buffer(2)]],
    device const uint *cell_of [[buffer(3)]],
    device const uint *remainders [[buffer(4)]],
    device uint *quanta [[buffer(5)]],
    uint id [[thread_position_in_grid]])
{
    if (id >= params[1]) {
        return;
    }
    uint cell = cell_of[id];
    if (resolved[RESOLVED * cell + 4u] != MODE_DEFICIT) {
        return;
    }
    uint start = cells[CELL * cell], len = cells[CELL * cell + 1u];
    uint deficit = resolved[RESOLVED * cell + 3u];
    d64 mine = u_word(remainders, 2u * id);
    uint rank = 0u;
    for (uint k = 0u; k < len; k++) {
        uint other = start + k;
        d64 theirs = u_word(remainders, 2u * other);
        rank += uint(theirs > mine || (theirs == mine && other < id));
    }
    quanta[id] += deficit / len + uint(rank < deficit % len);
}
