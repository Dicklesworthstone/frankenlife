// FrankenLife's field kernels (plan §11.1, FL-031): phase 2 of the tick,
// transport, decay and the external sources, in flife-field's reference
// arithmetic, bit for bit, on softfloat64.metal's binary64.
//
// Field values are F32 bit patterns throughout (`uint` here), never held in
// a `float`, so subnormals survive as the reference keeps them. A non-finite
// operand sets the fault word, and the host then hands the step back to the
// reference, which computes it in hardware F64.
//
// The host keeps every sequential F64 sum (the energy totals and each
// source's weight sum); a sum of thousands of terms in a fixed order is
// serial, and the host's F64 is far faster at it than one GPU thread.

constant uint CHANNELS = 32u;

// One source's descriptor, in 32-bit words. Its candidate cells are the
// rows × columns of `rows_at`/`columns_at` in the axes buffer, row-major,
// stored from entry `first` of the weights buffer.
constant uint DESCRIPTOR = 73u;
constant uint FIRST = 0u;
constant uint ROWS = 1u;
constant uint COLUMNS = 2u;
constant uint ROWS_AT = 3u;
constant uint COLUMNS_AT = 4u;
constant uint CENTER_X = 5u;
constant uint CENTER_Y = 7u;
constant uint DIRECTION = 9u;

// A candidate beyond the radius, and one whose displacement the reference
// refuses. Neither is a weight or an amount: both are finite.
constant ulong ABSENT = 0xFFFFFFFFFFFFFFFFul;
constant ulong REFUSED = 0xFFFFFFFFFFFFFFFEul;

// A candidate rank table entry for a row or column that is no candidate.
constant uint NOT_CANDIDATE = 0xFFFFFFFFu;

static inline d64 d_word(device const uint *words, uint at) {
    return (ulong(words[at + 1u]) << 32) | ulong(words[at]);
}

static inline bool f32_finite(uint bits) {
    return ((bits >> 23) & 0xFFu) != 0xFFu;
}

// One periodic five-point substep on every plane: transport.rs's
// `((c·u + r_x·(u_w + u_e)) + r_y·(u_s + u_n))`, rounded once to F32.
// params: width, height, cells, planes, center, r_x, r_y (binary64 as two
// words, low first).
kernel void field_stencil(
    device const uint *source [[buffer(0)]],
    device const uint *params [[buffer(1)]],
    device uint *target [[buffer(2)]],
    device atomic_uint *fault [[buffer(3)]],
    uint id [[thread_position_in_grid]])
{
    uint width = params[0], height = params[1], cells = params[2], planes = params[3];
    if (id >= cells * planes) {
        return;
    }
    uint plane = id / cells, cell = id % cells;
    uint x = cell % width, y = cell / width;
    uint west = x == 0u ? width - 1u : x - 1u;
    uint east = x + 1u == width ? 0u : x + 1u;
    uint south = y == 0u ? height - 1u : y - 1u;
    uint north = y + 1u == height ? 0u : y + 1u;
    uint base = plane * cells;
    uint u = source[base + cell];
    uint w = source[base + y * width + west];
    uint e = source[base + y * width + east];
    uint s = source[base + south * width + x];
    uint n = source[base + north * width + x];
    if (!(f32_finite(u) && f32_finite(w) && f32_finite(e) && f32_finite(s) && f32_finite(n))) {
        atomic_fetch_or_explicit(fault, 1u, memory_order_relaxed);
    }
    d64 value = d_add(
        d_add(d_mul(d_word(params, 4u), d_from_f32_bits(u)),
              d_mul(d_word(params, 6u), d_add(d_from_f32_bits(w), d_from_f32_bits(e)))),
        d_mul(d_word(params, 8u), d_add(d_from_f32_bits(s), d_from_f32_bits(n))));
    target[id] = d_to_f32_bits(value);
}

// grid.rs's `minimum_image(from, to, length)`; sets `refused` where the
// reference refuses.
static d64 minimum_image(d64 from, d64 to, d64 length, thread bool &refused) {
    bool positive = d_finite(length) && !d_sign(length) && (length << 1) != 0ul;
    bool from_wrapped = d_finite(from) && (!d_sign(from) || (from << 1) == 0ul) && d_lt(from, length);
    bool to_wrapped = d_finite(to) && (!d_sign(to) || (to << 1) == 0ul) && d_lt(to, length);
    if (!(positive && from_wrapped && to_wrapped)) {
        refused = true;
        return 0ul;
    }
    d64 midpoint = d_mul(HALF, length);
    d64 difference = d_sub(to, from);
    if (!d_lt(difference, midpoint)) {
        return d_sub(difference, length);
    }
    if (d_lt(difference, midpoint ^ D_SIGN)) {
        return d_add(difference, length);
    }
    return difference;
}

// Every candidate cell's raw weight: sources.rs's `SourceKernel::weight`.
// header: sources, entries, world width, world height, cell width, cell
// height, r², 2σ² (binary64 as two words from word 2).
kernel void source_weights(
    device const uint *header [[buffer(0)]],
    device const uint *descriptors [[buffer(1)]],
    device const uint *axes [[buffer(2)]],
    device uint *weights [[buffer(3)]],
    uint id [[thread_position_in_grid]])
{
    uint sources = header[0], entries = header[1];
    if (id >= entries) {
        return;
    }
    uint j = 0u;
    while (j + 1u < sources && descriptors[(j + 1u) * DESCRIPTOR + FIRST] <= id) {
        j += 1u;
    }
    device const uint *d = descriptors + j * DESCRIPTOR;
    uint local = id - d[FIRST];
    uint row = axes[d[ROWS_AT] + local / d[COLUMNS]];
    uint column = axes[d[COLUMNS_AT] + local % d[COLUMNS]];
    d64 cell_x = d_mul(d_add(d_from_u32(column), HALF), d_word(header, 6u));
    d64 cell_y = d_mul(d_add(d_from_u32(row), HALF), d_word(header, 8u));
    bool refused = false;
    d64 dx = minimum_image(d_word(d, CENTER_X), cell_x, d_word(header, 2u), refused);
    d64 dy = minimum_image(d_word(d, CENTER_Y), cell_y, d_word(header, 4u), refused);
    d64 weight;
    if (refused) {
        weight = REFUSED;
    } else {
        // Both squares and their sum are nonnegative, so bits order them.
        d64 distance_squared = d_add(d_mul(dx, dx), d_mul(dy, dy));
        weight = distance_squared > d_word(header, 10u)
            ? ABSENT
            : exp_neg(d_div(distance_squared, d_word(header, 12u)));
    }
    weights[2u * id] = uint(weight & 0xFFFFFFFFul);
    weights[2u * id + 1u] = uint(weight >> 32);
}

// Every moment value: decayed (transport.rs's `f32(f64(u) × factor)`), then
// each source's deposit in source order (sources.rs's `deposit_moment`,
// `f32(f64(m) + q·a)`). The amounts `q` are the host's: `deposit_energy`
// computes them for the energy plane, and the host writes each over its
// candidate's weight, leaving the absent ones marked.
// params: cells, width, height, sources, decays, factor.
kernel void source_moments(
    device const uint *diffused [[buffer(0)]],
    device const uint *params [[buffer(1)]],
    device const uint *descriptors [[buffer(2)]],
    device const uint *ranks [[buffer(3)]],
    device const uint *amounts [[buffer(4)]],
    device uint *moments [[buffer(5)]],
    device atomic_uint *fault [[buffer(6)]],
    uint id [[thread_position_in_grid]])
{
    uint cells = params[0], width = params[1], height = params[2], sources = params[3];
    if (id >= cells * CHANNELS) {
        return;
    }
    uint channel = id / cells, cell = id % cells;
    uint x = cell % width, y = cell / width;
    uint bits = diffused[cells + id];
    if (!f32_finite(bits)) {
        atomic_fetch_or_explicit(fault, 1u, memory_order_relaxed);
    }
    if (params[4] != 0u) {
        bits = d_to_f32_bits(d_mul(d_from_f32_bits(bits), d_word(params, 5u)));
    }
    for (uint j = 0u; j < sources; ++j) {
        device const uint *d = descriptors + j * DESCRIPTOR;
        d64 component = d_word(d, DIRECTION + 2u * channel);
        if ((component << 1) == 0ul) {
            continue;
        }
        uint column = ranks[j * (width + height) + x];
        uint row = ranks[j * (width + height) + width + y];
        if (column == NOT_CANDIDATE || row == NOT_CANDIDATE) {
            continue;
        }
        d64 amount = d_word(amounts, 2u * (d[FIRST] + row * d[COLUMNS] + column));
        if (amount == ABSENT) {
            continue;
        }
        if (!f32_finite(bits)) {
            atomic_fetch_or_explicit(fault, 1u, memory_order_relaxed);
        }
        bits = d_to_f32_bits(d_add(d_from_f32_bits(bits), d_mul(amount, component)));
    }
    moments[id] = bits;
}
