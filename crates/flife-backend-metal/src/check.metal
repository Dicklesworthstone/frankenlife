// FrankenLife's phase 10 field check (plan §4.2, §8.1, §11.1, FL-032):
// flife-field's invariants.rs (`check_cell`) for every cell, bit for bit, on
// softfloat64.metal's binary64, reduced per threadgroup to the first failing
// cell and the first cell with the largest positive cone residual.
//
// The field is planar F32 (the energy plane, then 32 moment planes of `cells`
// values each), read as bit patterns. A cell fails INV-01 when its energy is
// not finite or is below zero (`-0.0` is not), or a moment is not finite; it
// fails INV-02 when `max(0, sqrt(Σ m²) − e)`, the squares summed in channel
// order, exceeds `absolute + relative · e`. The host refuses a tolerance whose
// bound could overflow, so every value here is finite.

constant uint CHECK_MOMENTS = 32u;
constant uint CHECK_THREADS = 256u;
constant uint NO_CELL = 0xFFFFFFFFu;

static inline bool check_finite(uint bits) {
    return (bits & 0x7F800000u) != 0x7F800000u;
}

// `params`: the cells, then the tolerance's absolute and relative parts as
// binary64 word pairs, low first. Each threadgroup writes four words to
// `groups`: its first failing cell (or NO_CELL), its largest positive
// residual as a word pair, and that residual's first cell (or NO_CELL).
kernel void field_check(
    device const uint *field [[buffer(0)]],
    device const uint *params [[buffer(1)]],
    device uint *groups [[buffer(2)]],
    uint cell [[thread_position_in_grid]],
    uint lane [[thread_position_in_threadgroup]],
    uint group [[threadgroup_position_in_grid]])
{
    threadgroup uint failing[CHECK_THREADS];
    threadgroup ulong largest[CHECK_THREADS];
    threadgroup uint holder[CHECK_THREADS];
    uint cells = params[0];
    d64 absolute = (ulong(params[2]) << 32) | ulong(params[1]);
    d64 relative = (ulong(params[4]) << 32) | ulong(params[3]);

    uint fails = NO_CELL;
    ulong residual = 0ul;
    uint at = NO_CELL;
    if (cell < cells) {
        uint energy = field[cell];
        bool passes = check_finite(energy) && !((energy >> 31) != 0u && (energy << 1) != 0u);
        d64 squares = 0ul;
        for (uint channel = 0u; passes && channel < CHECK_MOMENTS; ++channel) {
            uint moment = field[(channel + 1u) * cells + cell];
            if (!check_finite(moment)) {
                passes = false;
            } else {
                d64 m = d_from_f32_bits(moment);
                squares = d_add(squares, d_mul(m, m));
            }
        }
        if (passes) {
            d64 e = d_from_f32_bits(energy);
            d64 own = d_max(d_sub(d_sqrt(squares), e), 0ul);
            d64 bound = d_add(absolute, d_mul(relative, e));
            if (d_lt(bound, own)) {
                passes = false;
            } else if (d_lt(0ul, own)) {
                residual = own;
                at = cell;
            }
        }
        if (!passes) {
            fails = cell;
        }
    }
    failing[lane] = fails;
    largest[lane] = residual;
    holder[lane] = at;
    threadgroup_barrier(mem_flags::mem_threadgroup);
    // A residual is a nonnegative binary64, so its bits order as its values.
    for (uint stride = CHECK_THREADS / 2u; stride > 0u; stride >>= 1u) {
        if (lane < stride) {
            uint other = lane + stride;
            failing[lane] = min(failing[lane], failing[other]);
            if (largest[other] > largest[lane]
                || (largest[other] == largest[lane] && holder[other] < holder[lane])) {
                largest[lane] = largest[other];
                holder[lane] = holder[other];
            }
        }
        threadgroup_barrier(mem_flags::mem_threadgroup);
    }
    if (lane == 0u) {
        groups[4u * group] = failing[0];
        groups[4u * group + 1u] = uint(largest[0] & 0xFFFFFFFFul);
        groups[4u * group + 2u] = uint(largest[0] >> 32);
        groups[4u * group + 3u] = holder[0];
    }
}
