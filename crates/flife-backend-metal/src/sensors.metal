// FrankenLife's sensor kernels (plan §6.1, §8.1 phase 3, FL-031): the
// positive-box-v1 pyramid's coarse levels and the log-linear-v1 samples, in
// flife-field's pyramid.rs arithmetic, bit for bit, on softfloat64.metal's
// binary64.
//
// Level 0 is the F32 field, read as bit patterns and divided by the cell
// area in binary64, as `PyramidView::density` reads it. The coarse levels
// are binary64 (two words, low first), planar [plane][y][x], one level after
// another in one buffer. softfloat64 turns every non-finite operand into a
// quiet NaN, so a non-finite field value makes every sample that reads it,
// directly or through a coarse level, a NaN, and the host refuses that
// sample as the reference refuses its own non-finite one.

constant uint PLANES = 33u;
constant d64 QUARTER = 0x3FD0000000000000ul;

// A query's descriptor, in words: its lower level, whether it blends the
// next, then the lower level's columns, rows and fractions, the upper
// level's, and the blend weight t.
constant uint QUERY = 20u;
constant uint Q_LEVEL = 0u;
constant uint Q_BLENDS = 1u;
constant uint Q_LOW = 2u;
constant uint Q_HIGH = 10u;
constant uint Q_T = 18u;
// Within a level's part: x0, x1, y0, y1, fx (two words), fy (two words).

// The sample parameters: queries, the field's cells per plane, the cell
// area (two words), then per level from 0: cells along x, cells per plane,
// its offset in the levels buffer in binary64 values, spacing x and y (two
// words each).
constant uint S_HEADER = 4u;
constant uint S_LEVEL = 7u;

static inline d64 d_word(device const uint *words, uint at) {
    return (ulong(words[at + 1u]) << 32) | ulong(words[at]);
}

static inline void d_store(device uint *words, uint at, d64 value) {
    words[at] = uint(value & 0xFFFFFFFFul);
    words[at + 1u] = uint(value >> 32);
}

// pyramid.rs's `average`: ¼((a + b) + (c + d)) over the 2×2 block at
// (2x, 2y), a and b its lower row, of each plane's densities.
static inline d64 quarter_sum(d64 a, d64 b, d64 c, d64 d) {
    return d_mul(QUARTER, d_add(d_add(a, b), d_add(c, d)));
}

// Level 1 from the field: each density is f64(u) / area.
// params: field cells along x, level-1 cells along x, along y, field cells
// per plane, area (two words).
kernel void pyramid_first(
    device const uint *field [[buffer(0)]],
    device const uint *params [[buffer(1)]],
    device uint *levels [[buffer(2)]],
    uint id [[thread_position_in_grid]])
{
    uint fine_x = params[0], coarse_x = params[1], coarse_y = params[2], cells = params[3];
    uint coarse = coarse_x * coarse_y;
    if (id >= coarse * PLANES) {
        return;
    }
    uint plane = id / coarse, cell = id % coarse;
    uint x = cell % coarse_x, y = cell / coarse_x;
    uint base = plane * cells + 2u * y * fine_x + 2u * x;
    uint a = field[base], b = field[base + 1u];
    uint c = field[base + fine_x], d = field[base + fine_x + 1u];
    d64 area = d_word(params, 4u);
    d64 value = quarter_sum(
        d_div(d_from_f32_bits(a), area), d_div(d_from_f32_bits(b), area),
        d_div(d_from_f32_bits(c), area), d_div(d_from_f32_bits(d), area));
    d_store(levels, 2u * id, value);
}

// Level l + 1 from level l, both in the levels buffer.
// params: level-l cells along x, level-(l+1) cells along x, along y, level-l
// cells per plane, level l's offset, level l+1's offset (in values).
kernel void pyramid_next(
    device const uint *params [[buffer(0)]],
    device uint *levels [[buffer(1)]],
    uint id [[thread_position_in_grid]])
{
    uint fine_x = params[0], coarse_x = params[1], coarse_y = params[2], cells = params[3];
    uint from = params[4], to = params[5];
    uint coarse = coarse_x * coarse_y;
    if (id >= coarse * PLANES) {
        return;
    }
    uint plane = id / coarse, cell = id % coarse;
    uint x = cell % coarse_x, y = cell / coarse_x;
    uint base = from + plane * cells + 2u * y * fine_x + 2u * x;
    d64 value = quarter_sum(
        d_word(levels, 2u * base), d_word(levels, 2u * (base + 1u)),
        d_word(levels, 2u * (base + fine_x)), d_word(levels, 2u * (base + fine_x + 1u)));
    d_store(levels, 2u * (to + plane * coarse + cell), value);
}

// `PyramidView::density`: level 0 from the field, the rest from the levels.
static d64 density(
    device const uint *field, device const uint *levels, device const uint *params,
    uint level, uint plane, uint x, uint y)
{
    device const uint *at = params + S_HEADER + level * S_LEVEL;
    uint width = at[0], cells = at[1];
    if (level == 0u) {
        return d_div(d_from_f32_bits(field[plane * cells + y * width + x]), d_word(params, 2u));
    }
    return d_word(levels, 2u * (at[2] + plane * cells + y * width + x));
}

// `PyramidView::sample_level` for one plane: the lerp-form bilinear value
// and its analytic derivatives.
static void sample_level(
    device const uint *field, device const uint *levels, device const uint *params,
    device const uint *query, uint part, uint level, uint plane,
    thread d64 &value, thread d64 &dx, thread d64 &dy)
{
    uint x0 = query[part], x1 = query[part + 1u], y0 = query[part + 2u], y1 = query[part + 3u];
    d64 fx = d_word(query, part + 4u), fy = d_word(query, part + 6u);
    d64 v00 = density(field, levels, params, level, plane, x0, y0);
    d64 v10 = density(field, levels, params, level, plane, x1, y0);
    d64 v01 = density(field, levels, params, level, plane, x0, y1);
    d64 v11 = density(field, levels, params, level, plane, x1, y1);
    d64 lower = d_add(v00, d_mul(fx, d_sub(v10, v00)));
    d64 upper = d_add(v01, d_mul(fx, d_sub(v11, v01)));
    d64 across_lower = d_sub(v10, v00), across_upper = d_sub(v11, v01);
    d64 up_left = d_sub(v01, v00), up_right = d_sub(v11, v10);
    device const uint *at = params + S_HEADER + level * S_LEVEL;
    value = d_add(lower, d_mul(fy, d_sub(upper, lower)));
    dx = d_div(d_add(across_lower, d_mul(fy, d_sub(across_upper, across_lower))), d_word(at, 3u));
    dy = d_div(d_add(up_left, d_mul(fx, d_sub(up_right, up_left))), d_word(at, 5u));
}

// `PyramidView::sample`: one query's plane at its lower level, blended with
// the next level's by t where t is not zero. Each query's samples are its
// 33 values, then 33 x-derivatives, then 33 y-derivatives.
kernel void sensor_sample(
    device const uint *field [[buffer(0)]],
    device const uint *levels [[buffer(1)]],
    device const uint *params [[buffer(2)]],
    device const uint *queries [[buffer(3)]],
    device uint *samples [[buffer(4)]],
    uint id [[thread_position_in_grid]])
{
    uint count = params[0];
    if (id >= count * PLANES) {
        return;
    }
    uint index = id / PLANES, plane = id % PLANES;
    device const uint *query = queries + index * QUERY;
    uint level = query[Q_LEVEL];
    d64 value, dx, dy;
    sample_level(field, levels, params, query, Q_LOW, level, plane, value, dx, dy);
    if (query[Q_BLENDS] != 0u) {
        d64 high_value, high_dx, high_dy;
        sample_level(field, levels, params, query, Q_HIGH, level + 1u, plane,
                     high_value, high_dx, high_dy);
        d64 t = d_word(query, Q_T);
        value = d_add(value, d_mul(t, d_sub(high_value, value)));
        dx = d_add(dx, d_mul(t, d_sub(high_dx, dx)));
        dy = d_add(dy, d_mul(t, d_sub(high_dy, dy)));
    }
    uint base = 2u * (index * 3u * PLANES + plane);
    d_store(samples, base, value);
    d_store(samples, base + 2u * PLANES, dx);
    d_store(samples, base + 4u * PLANES, dy);
}
