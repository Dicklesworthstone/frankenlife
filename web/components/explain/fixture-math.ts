// The fixture bench's arithmetic: FrankenLife's rules from plan §5.2–§5.3,
// §6.3, §7.4 and §13.5, written out in double precision for hand-sized
// numbers, beside the plausible wrong version each Appendix C fixture exists
// to catch. This is not FrankenLife's code; its Rust kernels are checked
// against the same answers by the tests each tab names.
//
// FIXTURES holds the answers as the plan states them. They are never derived
// from these functions; fixture-math.test.ts checks that the functions
// reproduce them. Two are the page's own: C.15's start (10, which the plan
// leaves open) and the halfway frame that follows from it (10 − 48 / 2 on a
// width-128 torus, 114).

/** The answers Appendix C states (COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md). */
export const FIXTURES = {
  c1: {
    cell: 10,
    requests: [8, 12],
    efficiencies: [0.5, 0.25],
    grants: [4, 6],
    gains: [2, 1.5],
    dissipated: 6.5,
  },
  c2: { energy: 2, absorbed: 3, gain: 0, requestedToxicity: 5, toxicity: 2, dissipated: 5, wrongRecorded: 8 },
  c3: { a: { e: 3, m: 3 }, b: { e: 5, m: -5 }, energy: 8, moment: -2 },
  c5: { parent: 10, cost: 1, share: 1 / 3, child: 3, kept: 6, dissipated: 1 },
  c9: { etaMax: 0.8, k: 4, b: 0.15, kappa: 0.2, perPacket: 0.639355310069, mixture: -0.047643210759 },
  c11: { logit: 8, linear: 0.99966465, tanhFirst: 0.73105853 },
  c15: { width: 128, from: 10, moved: 80, reverse: 48, halfwayWrong: 114 },
} as const;

// ---------------------------------------------------------------------------
// C.1, plan §5.2: simultaneous uptake.

export type Grant = {
  /** The proportional ideal, `α q`. */
  ideal: number;
  /** What the organism receives. */
  granted: number;
};

export type Uptake = {
  demand: number;
  /** `α = min(1, e / A)`; 0 when nothing is demanded. */
  alpha: number;
  grants: Grant[];
  /** Energy left in the cell. */
  remaining: number;
  /** Energy granted beyond what the cell held: zero under the rule. */
  created: number;
};

function roundHalfEven(value: number): number {
  const floor = Math.floor(value);
  const fraction = value - floor;
  if (fraction > 0.5) return floor + 1;
  if (fraction < 0.5) return floor;
  return floor % 2 === 0 ? floor : floor + 1;
}

/**
 * The rule: two passes over one snapshot. First every request is summed into
 * the cell's demand `A`; then each organism is granted `α q` with
 * `α = min(1, e / A)`, in whole quanta of size `quantum`, the leftovers going
 * to the largest fractional remainders, ties to the lower identity
 * (`cell-quantum-largest-remainder-v1`, crates/flife-field/src/uptake.rs).
 * `requests[i]` belongs to identity `i`: order is identity order, and the
 * result never depends on any other order.
 */
export function uptakeRule(cell: number, requests: readonly number[], quantum = 1): Uptake {
  const demand = requests.reduce((sum, request) => sum + request, 0);
  const alpha = demand > 0 ? Math.min(1, cell / demand) : 0;
  const quanta = Math.floor(cell / quantum + 1e-9);
  // Contended: the whole cell, in quanta. Uncontended: the demand, rounded to whole quanta.
  const total = demand > cell ? quanta : Math.min(quanta, roundHalfEven(demand / quantum));
  const ideals = requests.map((request) => (alpha * request) / quantum);
  // Each ideal is num / den: request · e / (A · quantum) when contended, request / quantum when not. With whole
  // numbers the floors and remainders are exact integers, so equal remainders tie exactly and go to the lower
  // identity; in floating point, 7·4/21 and 7·7/21 leave remainders that differ in the last bit.
  // The same test as `total`: with no demand at all nothing is contended (and A = 0 must not be a divisor).
  const contended = demand > cell;
  const den = (contended ? demand : 1) * quantum;
  const whole = [cell, quantum, ...requests].every(Number.isInteger) && Number.isSafeInteger(cell * Math.max(0, ...requests));
  const shares = ideals.map((ideal, id) => {
    if (whole) {
      const num = (requests[id] ?? 0) * (contended ? cell : 1);
      const rest = num % den;
      return { id, floor: (num - rest) / den, remainder: rest / den };
    }
    return { id, floor: Math.floor(ideal + 1e-9), remainder: ideal - Math.floor(ideal + 1e-9) };
  });
  let left = total - shares.reduce((sum, share) => sum + share.floor, 0);
  const counts = shares.map((share) => share.floor);
  if (left > 0) {
    const order = [...shares].sort((x, y) => y.remainder - x.remainder || x.id - y.id);
    for (const share of order) {
      if (left === 0) break;
      counts[share.id] = (counts[share.id] ?? 0) + 1;
      left -= 1;
    }
  } else if (left < 0) {
    // Exactly, this never happens: the floors sum to at most ⌊Σ ideals⌋, which is at most the total. Floating point
    // reaches it when the 1e-9 allowance floors an ideal just under k to k; the smallest remainders give the extra
    // quantum back, ties to the higher identity.
    const order = [...shares].sort((x, y) => x.remainder - y.remainder || y.id - x.id);
    for (const share of order) {
      if (left === 0) break;
      if ((counts[share.id] ?? 0) > 0) {
        counts[share.id] = (counts[share.id] ?? 0) - 1;
        left += 1;
      }
    }
  }
  const grants = requests.map((_, id) => ({ ideal: (ideals[id] ?? 0) * quantum, granted: (counts[id] ?? 0) * quantum }));
  const granted = grants.reduce((sum, grant) => sum + grant.granted, 0);
  return { demand, alpha, grants, remaining: cell - granted, created: 0 };
}

/**
 * The plausible wrong version, one pass in thread order: each organism in
 * `order` subtracts its request from the cell and clamps at zero. Who eats
 * depends on which thread ran first.
 */
export function uptakeInOrder(cell: number, requests: readonly number[], order: readonly number[]): Uptake {
  const demand = requests.reduce((sum, request) => sum + request, 0);
  const grants: Grant[] = requests.map(() => ({ ideal: Number.NaN, granted: 0 }));
  let remaining = cell;
  for (const id of order) {
    const take = Math.max(0, Math.min(requests[id] ?? 0, remaining));
    const grant = grants[id];
    if (grant) grant.granted = take;
    remaining -= take;
  }
  return { demand, alpha: Number.NaN, grants, remaining, created: 0 };
}

/**
 * The other wrong version: threads race, each reads the same cell energy and
 * takes up to its request from it, and the overspent cell is "repaired" by
 * clamping it to zero while every organism keeps what it took. Energy appears
 * from nothing.
 */
export function uptakeRacing(cell: number, requests: readonly number[]): Uptake {
  const demand = requests.reduce((sum, request) => sum + request, 0);
  const grants = requests.map((request) => ({ ideal: Number.NaN, granted: Math.max(0, Math.min(request, cell)) }));
  const granted = grants.reduce((sum, grant) => sum + grant.granted, 0);
  return { demand, alpha: Number.NaN, grants, remaining: Math.max(0, cell - granted), created: Math.max(0, granted - cell) };
}

/** Useful gains `η u` and dissipation `Σ (1 − η) u`, with no toxicity (C.1's chemistry). */
export function usefulGains(grants: readonly number[], efficiencies: readonly number[]): { gains: number[]; dissipated: number } {
  const gains = grants.map((grant, id) => grant * (efficiencies[id] ?? 0));
  const dissipated = grants.reduce((sum, grant, id) => sum + grant * (1 - (efficiencies[id] ?? 0)), 0);
  return { gains, dissipated };
}

/** The `index`-th permutation of `0..n` in a fixed pseudo-random sequence (no Math.random: the page renders the same everywhere). */
export function threadOrder(n: number, index: number): number[] {
  const order = Array.from({ length: n }, (_, id) => id);
  if (index === 0) return order;
  // mulberry32, seeded by the index.
  let state = (index * 0x9e3779b9) >>> 0;
  const next = () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  for (let i = n - 1; i > 0; i--) {
    const j = Math.floor(next() * (i + 1));
    const a = order[i] ?? i;
    order[i] = order[j] ?? j;
    order[j] = a;
  }
  return order;
}

// ---------------------------------------------------------------------------
// C.3, plan §4.2: a cell holds energy e ≥ 0 and a signed moment m with ‖m‖ ≤ e.

export type Vec = [number, number];

/** A packet in a two-dimensional slice of the 32 chemical channels. */
export type Packet = { e: number; m: Vec };

export const norm = (v: Vec) => Math.hypot(v[0], v[1]);

/** Mixing packets adds energy and adds moment: opposite chemistry cancels, energy never does. */
export function mix(packets: readonly Packet[]): Packet {
  return packets.reduce<Packet>(
    (cell, packet) => ({ e: cell.e + packet.e, m: [cell.m[0] + packet.m[0], cell.m[1] + packet.m[1]] }),
    { e: 0, m: [0, 0] },
  );
}

/** The wrong version: if the signed channels themselves were the food, the cell's food would be ‖Σ m‖. */
export function signedFood(packets: readonly Packet[]): number {
  return norm(mix(packets).m);
}

/** A packet of energy `e` whose moment points at `angle` (radians) with `‖m‖ = purity · e`. */
export function packet(e: number, angle: number, purity = 1): Packet {
  return { e, m: [purity * e * Math.cos(angle), purity * e * Math.sin(angle)] };
}

// ---------------------------------------------------------------------------
// C.2 and C.9, plan §5.3: bounded-affinity chemistry, per granted packet.

export type Chemistry = { etaMax: number; k: number; b: number; kappa: number };

export const C9_CHEMISTRY: Chemistry = {
  etaMax: FIXTURES.c9.etaMax,
  k: FIXTURES.c9.k,
  b: FIXTURES.c9.b,
  kappa: FIXTURES.c9.kappa,
};

/** `η = η_max tanh(k a·z − b)`: positive feeds, negative poisons. */
export function efficiency(dot: number, chemistry: Chemistry): number {
  return chemistry.etaMax * Math.tanh(chemistry.k * dot - chemistry.b);
}

export type Settlement = {
  absorbed: number;
  gain: number;
  requestedToxicity: number;
  /** `min(requested, E + G)`. */
  toxicity: number;
  /** The organism's change, `G − T`. */
  net: number;
  /** `U − G + T`. */
  dissipated: number;
};

/** Settle granted packets `{ u, dot }` (dot = a·z) for an organism holding `energy`, each packet on its own. */
export function settlePackets(packets: readonly { u: number; dot: number }[], energy: number, chemistry: Chemistry): Settlement {
  let absorbed = 0;
  let gain = 0;
  let harm = 0;
  for (const { u, dot } of packets) {
    const eta = efficiency(dot, chemistry);
    absorbed += u;
    gain += Math.max(eta, 0) * u;
    harm += Math.max(-eta, 0) * u;
  }
  return settle(absorbed, gain, chemistry.kappa * harm, energy);
}

/** The wrong version of C.9: the same rule applied once, to the packets' average composition. */
export function settleMixture(packets: readonly { u: number; dot: number }[], energy: number, chemistry: Chemistry): Settlement {
  const absorbed = packets.reduce((sum, { u }) => sum + u, 0);
  return settlePackets([{ u: absorbed, dot: mixtureDot(packets) }], energy, chemistry);
}

/** The average composition's component along the affinity, `a · (Σ u z / Σ u)`. */
export function mixtureDot(packets: readonly { u: number; dot: number }[]): number {
  const absorbed = packets.reduce((sum, { u }) => sum + u, 0);
  return absorbed > 0 ? packets.reduce((sum, { u, dot }) => sum + u * dot, 0) / absorbed : 0;
}

/** §5.3's settlement from its totals: toxicity clipped to `E + G`. */
export function settle(absorbed: number, gain: number, requestedToxicity: number, energy: number): Settlement {
  const toxicity = Math.min(requestedToxicity, energy + gain);
  return { absorbed, gain, requestedToxicity, toxicity, net: gain - toxicity, dissipated: absorbed - gain + toxicity };
}

/** C.2's wrong version: debit the requested toxicity, whatever the organism holds. */
export function settleRequested(absorbed: number, gain: number, requestedToxicity: number): Settlement {
  return {
    absorbed,
    gain,
    requestedToxicity,
    toxicity: requestedToxicity,
    net: gain - requestedToxicity,
    dissipated: absorbed - gain + requestedToxicity,
  };
}

// ---------------------------------------------------------------------------
// C.11, plan §6.2–§6.3: a linear output layer, sigmoid exactly once.

export const sigmoid = (x: number) => 1 / (1 + Math.exp(-x));

/** The rule: the final layer's logit goes straight to sigmoid. */
export const decodeLinear = (logit: number) => sigmoid(logit);

/** The wrong version: a hidden-style tanh on the output layer, then sigmoid. */
export const decodeTanhFirst = (logit: number) => sigmoid(Math.tanh(logit));

// ---------------------------------------------------------------------------
// C.15, plan §13.5: drawing between two render epochs on a torus.

/** `x` wrapped into `[0, width)`. */
export function wrap(x: number, width: number): number {
  return ((x % width) + width) % width;
}

/** The minimum-image displacement from `from` to `to`, in `[−width/2, width/2)`. */
export function minimumImage(from: number, to: number, width: number): number {
  return wrap(to - from + width / 2, width) - width / 2;
}

/** The wrong version: the position drawn at fraction `t` of the gap, along the minimum image. */
export function minimumImageBetween(from: number, to: number, width: number, t: number): number {
  return wrap(from + t * minimumImage(from, to, width), width);
}

/**
 * The rule (crates/flife-render/src/pairing.rs): interpolating is safe only if
 * no organism can have moved half the extent in the gap. `travelBound` is
 * `ticks · Δt · maximum speed`.
 */
export const unambiguous = (travelBound: number, width: number) => travelBound < width / 2;

// ---------------------------------------------------------------------------
// C.5, plan §7.4: a birth is one transaction.

export type Birth = { parent: number; child: number | null; dissipated: number; vanished: number };

/** The rule: on acceptance the split commits whole; on refusal nothing is charged. */
export function birthRule(parent: number, cost: number, share: number, accepted: boolean): Birth {
  if (!accepted) return { parent, child: null, dissipated: 0, vanished: 0 };
  const rest = parent - cost;
  const child = share * rest;
  return { parent: rest - child, child, dissipated: cost, vanished: 0 };
}

/** The wrong version: charge the cost and the endowment first, then find the child's allocation refused. */
export function birthChargeFirst(parent: number, cost: number, share: number, accepted: boolean): Birth {
  const committed = birthRule(parent, cost, share, true);
  if (accepted) return committed;
  return { parent: committed.parent, child: null, dissipated: cost, vanished: committed.child ?? 0 };
}
