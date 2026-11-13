// FL-032, second stage: phase 6's maintenance and motion on the device, one
// thread per organism, after softfloat64.metal. This is the stretch of
// flife-engine's settle_stages from the exhaustion check after chemistry to
// the one after motion: MaintenanceCosts::bill, MaintenanceBill::settle,
// uptake_ratio, MotionResponse::new, and Motion's acceleration, propose,
// settle and displace, operation for operation on software binary64.
//
// As in chemistry.metal, a thread that meets anything the reference would
// refuse, or a value softfloat64 cannot carry (an infinity, which it turns
// into a NaN), writes a nonzero status and nothing else the host may use:
// the host refuses the call and the reference decides the tick.

// params: [0] organisms, then word pairs: [1] seconds per tick; [3] basal,
// [5] sensor at unit scale, [7] the sensor unit scale and [9] neural costs;
// [11] the drag decay e^(-gamma dt) and [13] the force coefficient;
// [15] the maximum speed and [17] acceleration; [19] the motion cost;
// [21] the response norm limit; [23] the uptake capacity per second; and
// [25] the world's width and [27] height.
constant uint M_PARAMS = 29u;

constant uint M_TERMS = 33u;

// An organism: word pairs [0] its budget's remaining energy; [2] whether
// the budget is exhausted, [3] whether its controller was evaluated, and
// [4] its parameter count, as words; then word pairs [5] its sensing scale,
// [7] its absorbed uptake U, [9] its velocity and [13] position; [17] its
// 33 motion-response coefficients; [83] its 33 sensor gradients, x then y.
constant uint M_ORGANISM = 215u;
constant uint M_RESPONSE = 17u;
constant uint M_GRADIENTS = 83u;

// An outcome: [0] status; [1] the stage that exhausted the budget (0 none,
// 1 chemistry, 2 maintenance, 3 motion); word pairs [2] the maintenance
// requested and [4] paid, [6] the basal, [8] sensor and [10] controller
// shares paid; [12] the proposed and [16] applied velocities, [20] the
// motion cost requested and [22] paid; [24] whether the motion was scaled,
// a word; word pairs [25] the position after the stages, [29] the remaining
// energy; and [31] whether the budget is exhausted.
constant uint M_OUTCOME = 32u;

constant uint STATUS_SCALE = 1u;      // a sensing scale that is not a positive length
constant uint STATUS_BILL = 2u;       // a bill share that is not a finite nonnegative energy
constant uint STATUS_RATIO = 3u;      // an absorbed uptake or a ratio the device cannot form
constant uint STATUS_RESPONSE = 4u;   // a non-finite response or one past the norm limit
constant uint STATUS_GRADIENT = 5u;   // a non-finite sensor gradient
constant uint STATUS_VELOCITY = 6u;   // a non-finite velocity
constant uint STATUS_CLAMP = 7u;      // a clamp the device cannot carry
constant uint STATUS_COST = 8u;       // a motion cost that is not finite
constant uint STATUS_POSITION = 9u;   // a displaced position that is not finite

constant uint DIED_CHEMISTRY = 1u;
constant uint DIED_MAINTENANCE = 2u;
constant uint DIED_MOTION = 3u;

// 1 - EPSILON/2, the clamp's shrink, and how many shrinks the device
// follows before it refuses.
constant ulong SHRINK = 0x3FEFFFFFFFFFFFFFul;
constant uint CLAMP_SHRINKS = 64u;

static inline d64 m_load(device const uint *words, uint at) {
    return ulong(words[at]) | (ulong(words[at + 1u]) << 32);
}

static inline void m_store(device uint *words, uint at, d64 value) {
    words[at] = uint(value & 0xFFFFFFFFul);
    words[at + 1u] = uint(value >> 32);
}

// EnergyBudget::pay of a finite nonnegative request that has been through
// `energy` (+ 0.0).
static d64 m_pay(thread d64 &remaining, thread bool &exhausted, d64 requested) {
    d64 paid = d_min(requested, remaining);
    remaining = d_eq(paid, remaining) ? 0ul : d_sub_down(remaining, paid);
    if (d_eq(remaining, 0ul) && d_lt(0ul, paid)) {
        exhausted = true;
    }
    return paid;
}

// motion.rs's clamp_radius: v scaled radially so that v.v <= limit^2 as
// computed in F64. False for a square the device cannot carry or a shrink
// loop longer than CLAMP_SHRINKS.
static bool clamp_radius(thread d64 &x, thread d64 &y, d64 limit) {
    d64 bound = d_mul(limit, limit);
    d64 squared = d_add(d_mul(x, x), d_mul(y, y));
    if (!d_finite(squared)) {
        return false;
    }
    if (!d_lt(bound, squared)) {
        return true;
    }
    d64 scale = d_div(limit, d_sqrt(squared));
    x = d_mul(x, scale);
    y = d_mul(y, scale);
    for (uint shrinks = 0u;; ++shrinks) {
        if (!d_lt(bound, d_add(d_mul(x, x), d_mul(y, y)))) {
            return true;
        }
        if (shrinks == CLAMP_SHRINKS) {
            return false;
        }
        x = d_mul(x, SHRINK);
        y = d_mul(y, SHRINK);
    }
}

kernel void motion_settle(
    device const uint *params [[buffer(0)]],
    device const uint *organisms [[buffer(1)]],
    device uint *out [[buffer(2)]],
    uint index [[thread_position_in_grid]])
{
    if (index >= params[0]) {
        return;
    }
    d64 seconds = m_load(params, 1u);
    device const uint *organism = organisms + index * M_ORGANISM;
    device uint *outcome = out + index * M_OUTCOME;
    d64 remaining = m_load(organism, 0u);
    bool exhausted = organism[2] != 0u;
    d64 vx = m_load(organism, 9u);
    d64 vy = m_load(organism, 11u);
    d64 px = m_load(organism, 13u);
    d64 py = m_load(organism, 15u);
    for (uint k = 2u; k < 25u; ++k) {
        outcome[k] = 0u;
    }
    uint died = 0u;

    // Vitality::check after chemistry.
    if (exhausted) {
        died = DIED_CHEMISTRY;
    }

    if (died == 0u) {
        // MaintenanceCosts::bill.
        d64 scale = m_load(organism, 5u);
        if (!d_finite(scale) || !d_lt(0ul, scale)) {
            outcome[0] = STATUS_SCALE;
            return;
        }
        d64 basal = d_mul(seconds, m_load(params, 3u));
        d64 sensor = d_mul(seconds, d_mul(m_load(params, 5u), d_div(scale, m_load(params, 7u))));
        d64 controller = organism[3] != 0u ? d_mul(m_load(params, 9u), d_from_u32(organism[4])) : 0ul;
        // MaintenanceBill::settle: each share finite and nonnegative, the
        // total paid, and the payment split basal first, then sensor.
        if (!d_finite(basal) || d_lt(basal, 0ul) || !d_finite(sensor) || d_lt(sensor, 0ul) ||
            !d_finite(controller) || d_lt(controller, 0ul)) {
            outcome[0] = STATUS_BILL;
            return;
        }
        d64 total = d_add(d_add(d_add(basal, sensor), controller), 0ul);
        if (!d_finite(total)) {
            outcome[0] = STATUS_BILL;
            return;
        }
        d64 paid = m_pay(remaining, exhausted, total);
        d64 basal_paid = d_min(paid, basal);
        d64 rest = d_sub_down(paid, basal_paid);
        d64 sensor_paid = d_min(rest, sensor);
        m_store(outcome, 2u, total);
        m_store(outcome, 4u, paid);
        m_store(outcome, 6u, basal_paid);
        m_store(outcome, 8u, sensor_paid);
        m_store(outcome, 10u, d_sub_down(rest, sensor_paid));
        if (exhausted) {
            died = DIED_MAINTENANCE;
        }
    }

    if (died == 0u) {
        // uptake_ratio: U / (dt U_max), at most one; zero at zero capacity.
        d64 absorbed = m_load(organism, 7u);
        d64 capacity = m_load(params, 23u);
        if (!d_finite(absorbed) || d_lt(absorbed, 0ul)) {
            outcome[0] = STATUS_RATIO;
            return;
        }
        d64 ratio = 0ul;
        if (!d_eq(capacity, 0ul)) {
            d64 per_tick = d_mul(seconds, capacity);
            if (d_eq(per_tick, 0ul)) {
                outcome[0] = STATUS_RATIO;
                return;
            }
            ratio = d_min(d_div(absorbed, per_tick), ONE);
        }

        // MotionResponse::new: finite coefficients whose squares, summed in
        // order, are at most the limit's square.
        d64 squared = 0ul;
        for (uint t = 0u; t < M_TERMS; ++t) {
            d64 coefficient = m_load(organism, M_RESPONSE + 2u * t);
            if (!d_finite(coefficient)) {
                outcome[0] = STATUS_RESPONSE;
                return;
            }
            squared = d_add(squared, d_mul(coefficient, coefficient));
        }
        d64 limit = m_load(params, 21u);
        if (!d_finite(squared) || d_lt(d_mul(limit, limit), squared)) {
            outcome[0] = STATUS_RESPONSE;
            return;
        }

        // Motion::acceleration: g_u sum_j chi_j grad_j, energy first, then
        // the channels, clamped to the maximum acceleration.
        d64 sx = 0ul;
        d64 sy = 0ul;
        for (uint t = 0u; t < M_TERMS; ++t) {
            d64 coefficient = m_load(organism, M_RESPONSE + 2u * t);
            d64 gx = m_load(organism, M_GRADIENTS + 4u * t);
            d64 gy = m_load(organism, M_GRADIENTS + 4u * t + 2u);
            if (!d_finite(gx) || !d_finite(gy)) {
                outcome[0] = STATUS_GRADIENT;
                return;
            }
            sx = d_add(sx, d_mul(coefficient, gx));
            sy = d_add(sy, d_mul(coefficient, gy));
        }
        d64 ax = d_mul(ratio, sx);
        d64 ay = d_mul(ratio, sy);
        if (!clamp_radius(ax, ay, m_load(params, 17u))) {
            outcome[0] = STATUS_CLAMP;
            return;
        }

        // Motion::propose: e^(-gamma dt) v + coefficient f, clamped to the
        // maximum speed.
        if (!d_finite(vx) || !d_finite(vy) || !d_finite(ax) || !d_finite(ay)) {
            outcome[0] = STATUS_VELOCITY;
            return;
        }
        d64 decay = m_load(params, 11u);
        d64 force = m_load(params, 13u);
        d64 qx = d_add(d_mul(decay, vx), d_mul(force, ax));
        d64 qy = d_add(d_mul(decay, vy), d_mul(force, ay));
        if (!clamp_radius(qx, qy, m_load(params, 15u))) {
            outcome[0] = STATUS_CLAMP;
            return;
        }
        if (!d_finite(qx) || !d_finite(qy)) {
            outcome[0] = STATUS_VELOCITY;
            return;
        }

        // Motion::settle: c ||v||^2 dt, paid whole when affordable; otherwise
        // the balance is paid and the velocity scaled by sqrt(available /
        // requested).
        d64 requested = d_mul(d_mul(m_load(params, 19u), d_add(d_mul(qx, qx), d_mul(qy, qy))), seconds);
        if (!d_finite(requested) || d_lt(requested, 0ul)) {
            outcome[0] = STATUS_COST;
            return;
        }
        d64 available = remaining;
        d64 paid;
        bool scaled = d_lt(available, requested);
        if (!scaled) {
            requested = d_add(requested, 0ul);
            paid = m_pay(remaining, exhausted, requested);
            vx = qx;
            vy = qy;
        } else {
            d64 factor = d_sqrt(d_div(available, requested));
            paid = m_pay(remaining, exhausted, d_add(available, 0ul));
            vx = d_mul(qx, factor);
            vy = d_mul(qy, factor);
        }
        m_store(outcome, 12u, qx);
        m_store(outcome, 14u, qy);
        m_store(outcome, 16u, vx);
        m_store(outcome, 18u, vy);
        m_store(outcome, 20u, requested);
        m_store(outcome, 22u, paid);
        outcome[24] = scaled ? 1u : 0u;

        // Motion::displace: wrap(x + dt v) on the torus.
        px = d_add(px, d_mul(seconds, vx));
        py = d_add(py, d_mul(seconds, vy));
        if (!d_finite(px) || !d_finite(py)) {
            outcome[0] = STATUS_POSITION;
            return;
        }
        px = d_wrap(px, m_load(params, 25u));
        py = d_wrap(py, m_load(params, 27u));
        if (exhausted) {
            died = DIED_MOTION;
        }
    }

    outcome[0] = 0u;
    outcome[1] = died;
    m_store(outcome, 25u, px);
    m_store(outcome, 27u, py);
    m_store(outcome, 29u, remaining);
    outcome[31] = exhausted ? 1u : 0u;
}

// The arithmetic alone, for the tests: per operand pair (four words), a row
// of word pairs: sqrt(a), fmod(a, b), rem_euclid(a, b), and wrap_coordinate
// of a on the period |b| (a NaN where the reference refuses: a non-finite a
// or b, or b = 0).
constant uint M_OPS = 8u;

kernel void motion_ops(
    device const uint *operands [[buffer(0)]],
    device uint *out [[buffer(1)]],
    uint index [[thread_position_in_grid]])
{
    d64 a = m_load(operands, 4u * index);
    d64 b = m_load(operands, 4u * index + 2u);
    bool wraps = d_finite(a) && d_finite(b) && !d_eq(b, 0ul);
    d64 results[4] = {
        d_sqrt(a),
        d_fmod(a, b),
        d_rem_euclid(a, b),
        wraps ? d_wrap(a, b & ~D_SIGN) : D_QNAN,
    };
    device uint *row = out + index * M_OPS;
    for (uint k = 0u; k < 4u; ++k) {
        m_store(row, 2u * k, results[k]);
    }
}
