// FrankenLife's phase 9 kernels (plan §8.1, §11.1, FL-032): the uptake
// grants' debit and the deposits' reduction into the tick's candidate
// field, in flife-field's uptake.rs (`apply_grants_on`) and deposit.rs
// (`apply_deposits_on`) arithmetic, bit for bit, on softfloat64.metal's
// binary64.
//
// The field is planar F32 (the energy plane, then 32 moment planes of
// `cells` values each) read and written as bit patterns, so subnormal
// values keep theirs. Binary64 values are two words, low first.
// softfloat64 turns a non-finite operand into a quiet NaN where IEEE gives
// an infinity or zero, so a kernel that meets a non-finite field value, or
// would write one, raises the fault word and the host hands the tick to the
// reference.

constant uint MOMENTS = 32u;
// A debit, in words: its cell, its F32 energy before, its F32 energy after.
constant uint DEBIT = 3u;
// A grouped packet, in words: its binary64 amount, then its deposit's index.
constant uint PACKET = 3u;
// A deposit's direction: 32 binary64 components.
constant uint DIRECTION = 2u * MOMENTS;

static inline d64 r_load(device const uint *words, uint at) {
    return (ulong(words[at + 1u]) << 32) | ulong(words[at]);
}

static inline void r_store(device uint *words, uint at, d64 value) {
    words[at] = uint(value & 0xFFFFFFFFul);
    words[at + 1u] = uint(value >> 32);
}

static inline bool f32_finite(uint bits) {
    return (bits & 0x7F800000u) != 0x7F800000u;
}

// deposit.rs's `round_down_f32` of a finite `value >= 0`: the nearest F32,
// one step down when it lies above. An F32 overflow is infinity, above
// every finite value, so it steps down to the largest finite F32.
static uint round_down_f32(d64 value) {
    uint nearest = d_to_f32_bits(value);
    if (!f32_finite(nearest)) {
        return nearest - 1u;
    }
    return d_lt(value, d_from_f32_bits(nearest)) ? nearest - 1u : nearest;
}

// Each debit that granted energy, one thread each: its cell's energy
// becomes `after`, and each moment `m` becomes `(m * (after / before)) as
// f32`, or zero when `after` is zero. The host sends the debits in strictly
// ascending cell order, so no two threads share a cell.
kernel void grants_debit(
    device uint *field [[buffer(0)]],
    device const uint *debits [[buffer(1)]],
    device const uint *params [[buffer(2)]],
    device atomic_uint *fault [[buffer(3)]],
    uint index [[thread_position_in_grid]])
{
    uint count = params[0];
    uint cells = params[1];
    if (index >= count) {
        return;
    }
    uint cell = debits[index * DEBIT];
    uint before = debits[index * DEBIT + 1u];
    uint after = debits[index * DEBIT + 2u];
    if (cell >= cells || !f32_finite(before) || !f32_finite(after)) {
        atomic_store_explicit(fault, 1u, memory_order_relaxed);
        return;
    }
    field[cell] = after;
    // A drained cell's moments become zero whatever they held.
    if ((after << 1) == 0u) {
        for (uint channel = 0u; channel < MOMENTS; ++channel) {
            field[(channel + 1u) * cells + cell] = 0u;
        }
        return;
    }
    d64 ratio = d_div(d_from_f32_bits(after), d_from_f32_bits(before));
    for (uint channel = 0u; channel < MOMENTS; ++channel) {
        uint at = (channel + 1u) * cells + cell;
        uint moment = field[at];
        if (!f32_finite(moment)) {
            atomic_store_explicit(fault, 1u, memory_order_relaxed);
            return;
        }
        uint scaled = d_to_f32_bits(d_mul(d_from_f32_bits(moment), ratio));
        if (!f32_finite(scaled)) {
            atomic_store_explicit(fault, 1u, memory_order_relaxed);
            return;
        }
        field[at] = scaled;
    }
}

// Each cell, one thread each: its packets, in (cell, identity) order, are
// `offsets[cell]..offsets[cell + 1]`. Their amounts are summed rounding
// down, and each moment sum is `Σ amount * component`. The energy becomes
// the F32 at or below `before + total` (rounded down), what the cell
// received is that less `before`, and each moment rises by its sum times
// `received / total` when that product is not zero. A cell with packets
// writes what it received to `received`.
kernel void deposits_reduce(
    device uint *field [[buffer(0)]],
    device const uint *offsets [[buffer(1)]],
    device const uint *packets [[buffer(2)]],
    device const uint *directions [[buffer(3)]],
    device const uint *params [[buffer(4)]],
    device uint *received [[buffer(5)]],
    device atomic_uint *fault [[buffer(6)]],
    uint cell [[thread_position_in_grid]])
{
    uint cells = params[0];
    uint deposits = params[1];
    if (cell >= cells) {
        return;
    }
    uint first = offsets[cell];
    uint end = offsets[cell + 1u];
    if (first >= end) {
        return;
    }
    uint before_bits = field[cell];
    if (!f32_finite(before_bits)) {
        atomic_store_explicit(fault, 1u, memory_order_relaxed);
        return;
    }
    d64 total = 0ul;
    d64 moment[MOMENTS];
    for (uint channel = 0u; channel < MOMENTS; ++channel) {
        moment[channel] = 0ul;
    }
    for (uint at = first; at < end; ++at) {
        d64 amount = r_load(packets, at * PACKET);
        uint deposit = packets[at * PACKET + 2u];
        if (!d_finite(amount) || deposit >= deposits) {
            atomic_store_explicit(fault, 1u, memory_order_relaxed);
            return;
        }
        total = d_add_down(total, amount);
        for (uint channel = 0u; channel < MOMENTS; ++channel) {
            d64 component = r_load(directions, deposit * DIRECTION + 2u * channel);
            moment[channel] = d_add(moment[channel], d_mul(amount, component));
        }
    }
    d64 before = d_from_f32_bits(before_bits);
    uint after_bits = round_down_f32(d_add_down(before, total));
    d64 gained = d_sub(d_from_f32_bits(after_bits), before);
    d64 factor = d_lt(0ul, total) ? d_div(gained, total) : 0ul;
    if (!d_finite(total) || !f32_finite(after_bits) || !d_finite(factor)) {
        atomic_store_explicit(fault, 1u, memory_order_relaxed);
        return;
    }
    for (uint channel = 0u; channel < MOMENTS; ++channel) {
        uint at = (channel + 1u) * cells + cell;
        uint value = field[at];
        if (!f32_finite(value) || !d_finite(moment[channel])) {
            atomic_store_explicit(fault, 1u, memory_order_relaxed);
            return;
        }
        d64 added = d_mul(moment[channel], factor);
        if (!d_eq(added, 0ul)) {
            uint raised = d_to_f32_bits(d_add(d_from_f32_bits(value), added));
            if (!f32_finite(raised)) {
                atomic_store_explicit(fault, 1u, memory_order_relaxed);
                return;
            }
            field[at] = raised;
        }
    }
    field[cell] = after_bits;
    r_store(received, 2u * cell, gained);
}
