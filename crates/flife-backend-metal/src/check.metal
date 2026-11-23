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
constant uint CHECK_CARRIER0_MOMENTS = 24u;
constant uint CHECK_CARRIER1_ENERGY = 25u;
constant uint CHECK_CARRIER1_FIRST_MOMENT = 26u;
constant uint CHECK_CARRIER1_MOMENTS = 8u;
constant uint CHECK_THREADS = 256u;
constant uint NO_CELL = 0xFFFFFFFFu;

static inline bool check_finite(uint bits) {
    return (bits & 0x7F800000u) != 0x7F800000u;
}

static bool check_carrier(
    device const uint *field,
    uint cells,
    uint cell,
    uint energy_plane,
    uint first_moment_plane,
    uint moment_count,
    d64 absolute,
    d64 relative,
    thread d64 &own_out)
{
    uint energy = field[energy_plane * cells + cell];
    bool passes = check_finite(energy) && !((energy >> 31) != 0u && (energy << 1) != 0u);
    d64 squares = 0ul;
    for (uint channel = 0u; passes && channel < moment_count; ++channel) {
        uint moment = field[(first_moment_plane + channel) * cells + cell];
        if (!check_finite(moment)) {
            passes = false;
        } else {
            d64 m = d_from_f32_bits(moment);
            squares = d_add(squares, d_mul(m, m));
        }
    }
    if (!passes) {
        return false;
    }
    d64 e = d_from_f32_bits(energy);
    d64 own = d_max(d_sub(d_sqrt(squares), e), 0ul);
    d64 bound = d_add(absolute, d_mul(relative, e));
    if (d_lt(bound, own)) {
        return false;
    }
    own_out = own;
    return true;
}

// `params`: [0] cells, [1..3] absolute tolerance, [3..5] relative tolerance
// (binary64 word pairs, low first), [5] dual (0 = 33-plane single-carrier,
// 1 = 34-plane dual-carrier V2-15 checking Carrier 0 & Carrier 1 cones
// independently). Each threadgroup writes four words to `groups`: its first
// failing cell (or NO_CELL), its largest positive residual as a word pair,
// and that residual's first cell (or NO_CELL).
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
    bool dual = params[5] != 0u;

    uint fails = NO_CELL;
    ulong residual = 0ul;
    uint at = NO_CELL;
    if (cell < cells) {
        d64 own = 0ul;
        bool passes;
        if (!dual) {
            passes = check_carrier(
                field, cells, cell, 0u, 1u, CHECK_MOMENTS, absolute, relative, own);
        } else {
            d64 r0 = 0ul;
            d64 r1 = 0ul;
            passes = check_carrier(
                         field, cells, cell, 0u, 1u, CHECK_CARRIER0_MOMENTS, absolute, relative, r0)
                  && check_carrier(
                         field, cells, cell, CHECK_CARRIER1_ENERGY, CHECK_CARRIER1_FIRST_MOMENT,
                         CHECK_CARRIER1_MOMENTS, absolute, relative, r1);
            own = d_max(r0, r1);
        }
        if (!passes) {
            fails = cell;
        } else if (d_lt(0ul, own)) {
            residual = own;
            at = cell;
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
