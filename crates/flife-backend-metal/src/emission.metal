// FL-032, third stage: phase 6's emission on the device, one thread per
// organism that reaches it, after softfloat64.metal: flife-engine's
// EmissionPolicy::emit with flife-field's Deposits::push (its checks,
// ExchangeStencil::at and the deposit split) and EnergyBudget::pay,
// operation for operation on software binary64, then the exhaustion check
// after emission. The host stages each deposit's packets in identity order.
//
// As in chemistry.metal, a thread that meets anything the reference would
// refuse, or a value softfloat64 cannot carry, writes a nonzero status and
// the host refuses the call.

// params: [0] organisms, [1] the grid's columns and [2] rows; then word
// pairs: [3] the transfer efficiency, [5] the world's width and [7] height,
// [9] a cell's width and [11] height.
constant uint E_PARAMS = 13u;

// An organism: word pairs [0] its budget's remaining energy; [2] whether
// it emits, a word; word pairs [3] its spend fraction, [5] its position,
// and [9] its 32-channel direction.
constant uint E_ORGANISM = 73u;
constant uint E_DIRECTION = 9u;

// An outcome: [0] status; [1] whether emission exhausted the budget, [2]
// whether a deposit was staged, [3] its packet count; word pairs [4] the
// energy available, [6] the payment requested and [8] paid, [10] the offer,
// [12] the remaining energy; [14] whether the budget is exhausted; then up
// to four packets, each a cell word and an amount pair, from [15].
constant uint E_OUTCOME = 27u;
constant uint E_PACKETS = 15u;

constant uint E_CHANNELS = 32u;

constant uint STATUS_FRACTION = 1u;   // a spend fraction outside [0, 1]
constant uint STATUS_AMOUNT = 2u;     // a payment or offer the deposit refuses
constant uint STATUS_DIRECTION = 3u;  // a direction outside the unit ball
constant uint STATUS_POSITION = 4u;   // a position that is not finite

static inline d64 e_load(device const uint *words, uint at) {
    return ulong(words[at]) | (ulong(words[at + 1u]) << 32);
}

static inline void e_store(device uint *words, uint at, d64 value) {
    words[at] = uint(value & 0xFFFFFFFFul);
    words[at + 1u] = uint(value >> 32);
}

// flife-field's wrap_index: the Euclidean remainder of a cell coordinate.
static inline uint wrap_index(long coordinate, uint cells) {
    long n = long(cells);
    return uint(((coordinate % n) + n) % n);
}

// ledger::add_up, `-add_down(-a, -b)`.
static inline d64 d_add_up(d64 a, d64 b) {
    return d_add_down(a ^ D_SIGN, b ^ D_SIGN) ^ D_SIGN;
}

kernel void emission_settle(
    device const uint *params [[buffer(0)]],
    device const uint *organisms [[buffer(1)]],
    device uint *out [[buffer(2)]],
    uint index [[thread_position_in_grid]])
{
    if (index >= params[0]) {
        return;
    }
    uint cells_x = params[1];
    uint cells_y = params[2];
    device const uint *organism = organisms + index * E_ORGANISM;
    device uint *outcome = out + index * E_OUTCOME;
    for (uint k = 1u; k < E_OUTCOME; ++k) {
        outcome[k] = 0u;
    }
    d64 remaining = e_load(organism, 0u);
    bool exhausted = false;
    d64 available = remaining;
    e_store(outcome, 4u, available);

    if (organism[2] != 0u) {
        d64 fraction = e_load(organism, 3u);
        if (!d_finite(fraction) || d_lt(fraction, 0ul) || d_lt(ONE, fraction)) {
            outcome[0] = STATUS_FRACTION;
            return;
        }
        d64 requested = d_min(d_mul(fraction, available), available);
        if (!d_eq(requested, 0ul)) {
            d64 offered = d_mul(e_load(params, 3u), requested);

            // Deposits::push's checks.
            if (!d_finite(requested) || d_lt(requested, 0ul) || !d_finite(offered) ||
                d_lt(offered, 0ul) || d_lt(requested, offered)) {
                outcome[0] = STATUS_AMOUNT;
                return;
            }
            d64 squared = 0ul;
            for (uint c = 0u; c < E_CHANNELS; ++c) {
                d64 component = e_load(organism, E_DIRECTION + 2u * c);
                squared = d_add(squared, d_mul(component, component));
            }
            if (!d_finite(squared) || d_lt(ONE, squared)) {
                outcome[0] = STATUS_DIRECTION;
                return;
            }

            // ExchangeStencil::at: the bilinear weights of the four cells
            // around the wrapped position, zero weights skipped, a cell met
            // twice merged, then sorted by cell.
            d64 px = e_load(organism, 5u);
            d64 py = e_load(organism, 7u);
            if (!d_finite(px) || !d_finite(py)) {
                outcome[0] = STATUS_POSITION;
                return;
            }
            px = d_wrap(px, e_load(params, 5u));
            py = d_wrap(py, e_load(params, 7u));
            d64 cx = d_sub(d_div(px, e_load(params, 9u)), HALF);
            d64 cy = d_sub(d_div(py, e_load(params, 11u)), HALF);
            d64 floor_x = d_floor(cx);
            d64 floor_y = d_floor(cy);
            d64 fx = d_sub(cx, floor_x);
            d64 fy = d_sub(cy, floor_y);
            long column = d_to_long(floor_x);
            long row = d_to_long(floor_y);
            uint cells[4] = {0u, 0u, 0u, 0u};
            d64 weights[4] = {0ul, 0ul, 0ul, 0ul};
            uint len = 0u;
            for (uint corner = 0u; corner < 4u; ++corner) {
                uint dx = corner & 1u;
                uint dy = corner >> 1;
                d64 weight = d_mul(dx != 0u ? fx : d_sub(ONE, fx), dy != 0u ? fy : d_sub(ONE, fy));
                if (d_eq(weight, 0ul)) {
                    continue;
                }
                uint cell = wrap_index(row + long(dy), cells_y) * cells_x +
                            wrap_index(column + long(dx), cells_x);
                uint slot = len;
                for (uint s = 0u; s < len; ++s) {
                    if (cells[s] == cell) {
                        slot = s;
                        break;
                    }
                }
                if (slot < len) {
                    weights[slot] = d_add(weights[slot], weight);
                } else {
                    cells[len] = cell;
                    weights[len] = weight;
                    len += 1u;
                }
            }
            for (uint i = 1u; i < len; ++i) {
                for (uint j = i; j > 0u && cells[j - 1u] > cells[j]; --j) {
                    uint c = cells[j - 1u];
                    cells[j - 1u] = cells[j];
                    cells[j] = c;
                    d64 w = weights[j - 1u];
                    weights[j - 1u] = weights[j];
                    weights[j] = w;
                }
            }

            // The deposit split: every cell but the first of the largest
            // weight takes offered times its weight, summed rounding up;
            // that cell takes what is left, rounded down, at least zero.
            uint largest = 0u;
            for (uint i = 0u; i < len; ++i) {
                if (d_lt(weights[largest], weights[i])) {
                    largest = i;
                }
            }
            d64 shares[4] = {0ul, 0ul, 0ul, 0ul};
            d64 others = 0ul;
            for (uint i = 0u; i < len; ++i) {
                if (i != largest) {
                    shares[i] = d_mul(offered, weights[i]);
                    others = d_add_up(others, shares[i]);
                }
            }
            shares[largest] = d_max(d_sub_down(offered, others), 0ul);
            uint count = 0u;
            for (uint i = 0u; i < len; ++i) {
                if (d_lt(0ul, shares[i])) {
                    outcome[E_PACKETS + 3u * count] = cells[i];
                    e_store(outcome, E_PACKETS + 3u * count + 1u, shares[i]);
                    count += 1u;
                }
            }

            // EnergyBudget::pay of the request.
            requested = d_add(requested, 0ul);
            d64 paid = d_min(requested, remaining);
            remaining = d_eq(paid, remaining) ? 0ul : d_sub_down(remaining, paid);
            if (d_eq(remaining, 0ul) && d_lt(0ul, paid)) {
                exhausted = true;
            }
            outcome[2] = 1u;
            outcome[3] = count;
            e_store(outcome, 6u, requested);
            e_store(outcome, 8u, paid);
            e_store(outcome, 10u, offered);
        }
    }

    outcome[0] = 0u;
    outcome[1] = exhausted ? 1u : 0u;
    e_store(outcome, 12u, remaining);
    outcome[14] = exhausted ? 1u : 0u;
}

// The arithmetic alone, for the tests: per operand (two words), a row of
// word pairs: floor(a), a as an integer after floor (two words, low first),
// and add_up(a, a / 3).
constant uint E_OPS = 6u;

kernel void emission_ops(
    device const uint *operands [[buffer(0)]],
    device uint *out [[buffer(1)]],
    uint index [[thread_position_in_grid]])
{
    d64 a = e_load(operands, 2u * index);
    d64 floored = d_floor(a);
    device uint *row = out + index * E_OPS;
    e_store(row, 0u, floored);
    bool small = d_finite(a) && (floored & ~D_SIGN) < 0x43D0000000000000ul;  // below 2^62
    e_store(row, 2u, small ? ulong(d_to_long(floored)) : 0ul);
    e_store(row, 4u, d_add_up(a, d_div(a, 0x4008000000000000ul)));
}
