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

constant ulong VASC_RATIO_CAP = 0x54B249AD2594C37Dul;   // 1.0e100
constant ulong ONE_POINT_FIVE = 0x3FF8000000000000ul;   // 1.5
constant uint CARRIER1_ENERGY_PLANE = 25u;
constant uint CARRIER1_TOTAL_PLANES = 9u;               // e_1 + 8 trail moments

// One source's descriptor, in 32-bit words. Its candidate cells are the
// rows × columns of `rows_at`/`columns_at` in the axes buffer, row-major,
// stored from entry `first` of the weights buffer. Offsets 73..77 hold the
// per-source kernel constants (r_j^2, 2*sigma_j^2) for V2-02 heterogeneous
// oases.
constant uint DESCRIPTOR = 77u;
constant uint FIRST = 0u;
constant uint ROWS = 1u;
constant uint COLUMNS = 2u;
constant uint ROWS_AT = 3u;
constant uint COLUMNS_AT = 4u;
constant uint CENTER_X = 5u;
constant uint CENTER_Y = 7u;
constant uint DIRECTION = 9u;
constant uint RADIUS_SQUARED = 73u;
constant uint TWO_SIGMA_SQUARED = 75u;

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

// One periodic five-point substep on planes `first_plane..first_plane + plane_count`:
// transport.rs's `((c·u + r_x·(u_w + u_e)) + r_y·(u_s + u_n))`, rounded once to F32.
// params: [0] width, [1] height, [2] cells, [3] first_plane, [4] plane_count,
// then binary64 word pairs (low first): [5] center, [7] r_x, [9] r_y.
kernel void field_stencil(
    device const uint *source [[buffer(0)]],
    device const uint *params [[buffer(1)]],
    device uint *target [[buffer(2)]],
    device atomic_uint *fault [[buffer(3)]],
    uint id [[thread_position_in_grid]])
{
    uint width = params[0], height = params[1], cells = params[2];
    uint first_plane = params[3], plane_count = params[4];
    if (id >= cells * plane_count) {
        return;
    }
    uint plane = first_plane + id / cells, cell = id % cells;
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
        d_add(d_mul(d_word(params, 5u), d_from_f32_bits(u)),
              d_mul(d_word(params, 7u), d_add(d_from_f32_bits(w), d_from_f32_bits(e)))),
        d_mul(d_word(params, 9u), d_add(d_from_f32_bits(s), d_from_f32_bits(n))));
    target[base + cell] = d_to_f32_bits(value);
}

// One periodic flux-form doubly-stochastic substep over `plane_range`
// (`first_plane..first_plane + plane_count`) for V2-09 tight junctions
// (and reusable by V2-14 vascular fields and V2-15 dual-carrier splits).
// Evaluates transport.rs's `flux_cell` in exact IEEE-754 order:
//   center = (1.0 - (r_w + r_e)) - (r_s + r_n)
//   next   = ((center * u_c + (r_w * u_w + r_e * u_e)) + (r_s * u_s + r_n * u_n)) as f32
// params: [0] width, [1] height, [2] cells, [3] first_plane, [4] plane_count.
// edge_x/edge_y hold `cells` binary64 conductances (two 32-bit words each, low first).
kernel void field_stencil_flux(
    device const uint *source [[buffer(0)]],
    device const uint *params [[buffer(1)]],
    device const uint *edge_x [[buffer(2)]],
    device const uint *edge_y [[buffer(3)]],
    device uint *target [[buffer(4)]],
    device atomic_uint *fault [[buffer(5)]],
    uint id [[thread_position_in_grid]])
{
    uint width = params[0], height = params[1], cells = params[2];
    uint first_plane = params[3], plane_count = params[4];
    if (id >= cells * plane_count) {
        return;
    }
    uint plane = first_plane + id / cells;
    uint cell = id % cells;
    uint x = cell % width, y = cell / width;
    uint west = x == 0u ? width - 1u : x - 1u;
    uint east = x + 1u == width ? 0u : x + 1u;
    uint south = y == 0u ? height - 1u : y - 1u;
    uint north = y + 1u == height ? 0u : y + 1u;
    uint base = plane * cells;
    uint w_cell = y * width + west;
    uint e_cell = y * width + east;
    uint s_cell = south * width + x;
    uint n_cell = north * width + x;
    uint u_c = source[base + cell];
    uint u_w = source[base + w_cell];
    uint u_e = source[base + e_cell];
    uint u_s = source[base + s_cell];
    uint u_n = source[base + n_cell];
    if (!(f32_finite(u_c) && f32_finite(u_w) && f32_finite(u_e) && f32_finite(u_s) && f32_finite(u_n))) {
        atomic_fetch_or_explicit(fault, 1u, memory_order_relaxed);
    }
    d64 r_w = d_word(edge_x, 2u * w_cell);
    d64 r_e = d_word(edge_x, 2u * cell);
    d64 r_s = d_word(edge_y, 2u * s_cell);
    d64 r_n = d_word(edge_y, 2u * cell);
    d64 center = d_sub(d_sub(ONE, d_add(r_w, r_e)), d_add(r_s, r_n));
    d64 x_term = d_add(d_mul(r_w, d_from_f32_bits(u_w)), d_mul(r_e, d_from_f32_bits(u_e)));
    d64 y_term = d_add(d_mul(r_s, d_from_f32_bits(u_s)), d_mul(r_n, d_from_f32_bits(u_n)));
    d64 value = d_add(d_add(d_mul(center, d_from_f32_bits(u_c)), x_term), y_term);
    uint out_bits = d_to_f32_bits(value);
    if (!f32_finite(out_bits)) {
        atomic_fetch_or_explicit(fault, 1u, memory_order_relaxed);
    }
    target[base + cell] = out_bits;
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

// Every candidate cell's raw weight: sources.rs's `SourceKernel::weight`,
// reading per-source `(r_j², 2σ_j²)` from the 77-word descriptor.
// header: sources, entries, world width, world height, cell width, cell height
// (binary64 as two words from word 2).
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
    if (d[ROWS] == 0u || d[COLUMNS] == 0u) {
        return;
    }
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
        weight = distance_squared > d_word(d, RADIUS_SQUARED)
            ? ABSENT
            : exp_neg(d_div(distance_squared, d_word(d, TWO_SIGMA_SQUARED)));
    }
    weights[2u * id] = uint(weight & 0xFFFFFFFFul);
    weights[2u * id + 1u] = uint(weight >> 32);
}

// Every Carrier-0 moment value: decayed (transport.rs's `f32(f64(u) × factor)`),
// then each source's deposit in source order (sources.rs's `deposit_moment`,
// `f32(f64(m) + q·a)`). Runs over `moment_channels` (`32` single-carrier or `24`
// dual-carrier, leaving Carrier 1 untouched).
// params: [0] cells, [1] width, [2] height, [3] sources, [4] decays,
//         [5..7] factor (2 words), [7] fused, [8] moment_channels.
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
    uint moment_channels = params[8];
    if (id >= cells * moment_channels) {
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
    bool fused = params[7] != 0u;
    if (fused) {
        d64 delta = 0ul;
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
            delta = d_add(delta, d_mul(amount, component));
        }
        if ((delta << 1) != 0ul) {
            if (!f32_finite(bits)) {
                atomic_fetch_or_explicit(fault, 1u, memory_order_relaxed);
            }
            bits = d_to_f32_bits(d_add(d_from_f32_bits(bits), delta));
            if (!f32_finite(bits)) {
                atomic_fetch_or_explicit(fault, 1u, memory_order_relaxed);
            }
        }
    } else {
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
            if (!f32_finite(bits)) {
                atomic_fetch_or_explicit(fault, 1u, memory_order_relaxed);
            }
        }
    }
    moments[id] = bits;
}

// Carrier-1 (`V2-15` dual-carrier-trails) readback staging: copies plane 25
// (`e_1`, undecayed so host runs exact sequential F64 `energy_report`) into
// `trail_out[0..cells]` and decays planes 26..34 (8 trail moment planes) into
// `trail_out[cells..9*cells]` using `trail_plan.decay_factor()`.
// params: [0] cells, [1] decays, [2..4] factor (2 words).
kernel void trail_decay(
    device const uint *diffused [[buffer(0)]],
    device const uint *params [[buffer(1)]],
    device uint *trail_out [[buffer(2)]],
    device atomic_uint *fault [[buffer(3)]],
    uint id [[thread_position_in_grid]])
{
    uint cells = params[0];
    if (id >= cells * CARRIER1_TOTAL_PLANES) {
        return;
    }
    uint plane_offset = id / cells;
    uint cell = id % cells;
    uint bits = diffused[(CARRIER1_ENERGY_PLANE + plane_offset) * cells + cell];
    if (!f32_finite(bits)) {
        atomic_fetch_or_explicit(fault, 1u, memory_order_relaxed);
    }
    if (plane_offset > 0u && params[1] != 0u) {
        bits = d_to_f32_bits(d_mul(d_from_f32_bits(bits), d_word(params, 2u)));
        if (!f32_finite(bits)) {
            atomic_fetch_or_explicit(fault, 1u, memory_order_relaxed);
        }
    }
    trail_out[id] = bits;
}

// One edge's exact Murray / Physarum conductance update (`V2-14`), matching
// `VascularField::update_conductance` in transport.rs bit-for-bit on binary64:
// overflow guard (`ratio > 1e100 -> sat = 1.0`), `mu == 1.5` fast path
// (`ratio * sqrt(ratio)`), and general `mu > 0` via shared `d_pow_positive`.
static uint vascular_step(
    uint prev_bits,
    d64 flux,
    d64 dt,
    d64 g_min,
    d64 alpha_v,
    d64 gamma_v,
    d64 j0,
    d64 mu,
    device atomic_uint *fault)
{
    d64 sat;
    if (!d_finite(flux)) {
        // Finite nonnegative operands only lose finiteness via overflow (+inf).
        sat = ONE;
    } else {
        d64 ratio = d_div(flux, j0);
        if (!d_finite(ratio) || d_lt(VASC_RATIO_CAP, ratio)) {
            sat = ONE;
        } else if (!d_lt(0ul, ratio)) {
            sat = 0ul;
        } else if (mu == ONE_POINT_FIVE) {
            d64 r_mu = d_mul(ratio, d_sqrt(ratio));
            if (d_finite(r_mu)) {
                d64 denom = d_add(ONE, r_mu);
                sat = d_finite(denom) ? d_div(r_mu, denom) : ONE;
            } else {
                sat = ONE;
            }
        } else {
            d64 r_mu = 0ul;
            if (d_pow_positive(ratio, mu, r_mu) && d_finite(r_mu)) {
                d64 denom = d_add(ONE, r_mu);
                sat = d_finite(denom) ? d_div(r_mu, denom) : ONE;
            } else {
                sat = !d_lt(ratio, ONE) ? ONE : 0ul;
            }
        }
    }
    d64 g = d_from_f32_bits(prev_bits);
    d64 reinforce = d_mul(d_mul(alpha_v, sat), d_sub(ONE, g));
    d64 prune = d_mul(gamma_v, d_sub(g, g_min));
    d64 updated = d_add(g, d_mul(dt, d_sub(reinforce, prune)));
    if (!d_finite(updated)) {
        atomic_fetch_or_explicit(fault, 1u, memory_order_relaxed);
        return prev_bits;
    }
    d64 clamped = d_min(d_max(updated, g_min), ONE);
    return d_to_f32_bits(clamped);
}

// 4-neighbor flux-reinforced Murray / Physarum edge conductance update (`V2-14`):
// one thread per cell updates both east (`next_x[cell]`) and north (`next_y[cell]`)
// candidate edge conductances from pre-transport committed Carrier-0 energy `e_0`
// (`field[0..cells]`), precomputed `edge_x`/`edge_y`, and committed `prev_x`/`prev_y`.
// params: [0] width, [1] height, [2] cells, then binary64 word pairs (low first):
//         [3] substeps, [5] dt, [7] g_min, [9] alpha_v, [11] gamma_v, [13] j0, [15] mu.
kernel void vascular_update(
    device const uint *field [[buffer(0)]],
    device const uint *params [[buffer(1)]],
    device const uint *edge_x [[buffer(2)]],
    device const uint *edge_y [[buffer(3)]],
    device const uint *prev_x [[buffer(4)]],
    device const uint *prev_y [[buffer(5)]],
    device uint *next_x [[buffer(6)]],
    device uint *next_y [[buffer(7)]],
    device atomic_uint *fault [[buffer(8)]],
    uint cell [[thread_position_in_grid]])
{
    uint width = params[0], height = params[1], cells = params[2];
    if (cell >= cells) {
        return;
    }
    uint x = cell % width, y = cell / width;
    uint east = x + 1u == width ? 0u : x + 1u;
    uint north = y + 1u == height ? 0u : y + 1u;
    uint ec_bits = field[cell];
    uint ee_bits = field[y * width + east];
    uint en_bits = field[north * width + x];
    uint px_bits = prev_x[cell];
    uint py_bits = prev_y[cell];
    if (!(f32_finite(ec_bits) && f32_finite(ee_bits) && f32_finite(en_bits)
          && f32_finite(px_bits) && f32_finite(py_bits))) {
        atomic_fetch_or_explicit(fault, 1u, memory_order_relaxed);
        return;
    }
    d64 substeps = d_word(params, 3u);
    d64 dt = d_word(params, 5u);
    d64 g_min = d_word(params, 7u);
    d64 alpha_v = d_word(params, 9u);
    d64 gamma_v = d_word(params, 11u);
    d64 j0 = d_word(params, 13u);
    d64 mu = d_word(params, 15u);

    d64 ec = d_from_f32_bits(ec_bits);
    d64 ee = d_from_f32_bits(ee_bits);
    d64 en = d_from_f32_bits(en_bits);
    d64 diff_x = d_sub(ec, ee) & ~D_SIGN;
    d64 diff_y = d_sub(ec, en) & ~D_SIGN;
    d64 rate_scale = d_div(substeps, dt);
    d64 flux_x = d_mul(d_mul(d_word(edge_x, 2u * cell), rate_scale), diff_x);
    d64 flux_y = d_mul(d_mul(d_word(edge_y, 2u * cell), rate_scale), diff_y);
    next_x[cell] = vascular_step(px_bits, flux_x, dt, g_min, alpha_v, gamma_v, j0, mu, fault);
    next_y[cell] = vascular_step(py_bits, flux_y, dt, g_min, alpha_v, gamma_v, j0, mu, fault);
}
