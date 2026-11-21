// The controller's shape, as the code states it. Pure data and arithmetic for
// components/explain/brain.tsx, weights.tsx and scale.tsx.
//
// - Sensor ABI `energy-moment-gradient-104-v1` (crates/flife-neural/src/sensors.rs):
//   `[q, Z_0, …, Z_31]`, their x derivatives in that order, their y derivatives
//   in that order, then five intrinsic inputs.
// - Action ABI (crates/flife-neural/src/actions.rs): output 0 reproduction, 1
//   child share, 2 emission gate, 3–34 emission direction, 35 emission spending.
// - Catalog `dense-104-36-catalog-v1` (crates/flife-genome/src/catalog.rs,
//   plan §6.4): hidden widths [8], [32], [64, 32], [96, 64], [128, 96].

export const SENSOR_WIDTH = 104;
export const ACTION_WIDTH = 36;

export type Group = {
  id: string;
  /** Short name drawn beside the layer. */
  name: string;
  /** Its symbol, as the code and plan write it. */
  symbol: string;
  /** The label drawn beside its nodes. */
  short: string;
  size: number;
  /** One line on what it is. */
  description: string;
};

/** The 104 inputs in their fixed order (sensors.rs). */
export const INPUT_GROUPS: readonly Group[] = [
  {
    id: "q",
    name: "energy",
    symbol: "q",
    short: "q",
    size: 1,
    description:
      "The field's energy density around it at its own inherited sensing scale, q = R/(R + R_scale), read from the density pyramid.",
  },
  {
    id: "z",
    name: "composition",
    symbol: "Z₀…Z₃₁",
    short: "Z ×32",
    size: 32,
    description:
      "The chemistry around it: one regularized composition value per channel, Z = M/(R + R_floor). Signed: a channel can point for or against.",
  },
  {
    id: "dx",
    name: "x gradients",
    symbol: "tanh(L ∂/∂x)",
    short: "∂x ×33",
    size: 33,
    description:
      "How the energy and each of the 32 channels change along x, saturated with tanh. Motion follows the unsaturated gradients; the network sees these.",
  },
  {
    id: "dy",
    name: "y gradients",
    symbol: "tanh(L ∂/∂y)",
    short: "∂y ×33",
    size: 33,
    description: "The same 33 gradients along y.",
  },
  {
    id: "self",
    name: "itself",
    symbol: "E, age, vₓ, v_y, ΔE",
    short: "self ×5",
    size: 5,
    description:
      "Five intrinsic inputs: its own energy E/(E + E_scale), its age, its velocity in x and in y, and its recent rate of energy change.",
  },
];

/** The 36 outputs in decoding order (actions.rs). Sigmoid is applied once, in decoding. */
export const OUTPUT_GROUPS: readonly Group[] = [
  {
    id: "lambda",
    name: "reproduction",
    symbol: "λ",
    short: "λ split",
    size: 1,
    description:
      "An attempt rate λ = λ_max σ(o). The chance of trying to split this tick is p = 1 − e^(−λΔt), computed through a stable expm1.",
  },
  {
    id: "beta",
    name: "child share",
    symbol: "β",
    short: "β share",
    size: 1,
    description:
      "β = β_min + (β_max − β_min) σ(o): the share of the energy left after the birth cost that the child receives.",
  },
  {
    id: "gate",
    name: "emission gate",
    symbol: "σ(o)",
    short: "gate",
    size: 1,
    description: "Whether to emit at all: emission happens only when σ(o) is above the gate threshold θ_emit.",
  },
  {
    id: "direction",
    name: "emission direction",
    symbol: "32-D",
    short: "dir ×32",
    size: 32,
    description:
      "32 raw outputs normalized to a unit vector: the chemical composition it writes into the field, which its neighbours sense on the next tick.",
  },
  {
    id: "spend",
    name: "emission spending",
    symbol: "f",
    short: "f spend",
    size: 1,
    description:
      "f = 1 − e^(−r_max σ(o) Δt): the fraction of the energy it holds after moving that it pays to emit.",
  },
];

export type Topology = {
  /** Hidden-layer widths, catalog order. */
  hidden: readonly number[];
};

/** `dense-104-36-catalog-v1`, exactly plan §6.4's five rows in order. */
export const CATALOG: readonly Topology[] = [
  { hidden: [8] },
  { hidden: [32] },
  { hidden: [64, 32] },
  { hidden: [96, 64] },
  { hidden: [128, 96] },
];

/** The founders' topology in the presets: 104 → 64 → 32 → 36. */
export const DEFAULT_TOPOLOGY = 2;

/** Every layer's width, inputs to outputs. */
export function widths(topology: Topology): number[] {
  return [SENSOR_WIDTH, ...topology.hidden, ACTION_WIDTH];
}

/** `104 → 64 → 32 → 36`. */
export function shape(topology: Topology): string {
  return widths(topology).join(" → ");
}

/** One weight matrix and bias vector per layer: Σ (in × out + out). */
export function parameterCount(layerWidths: readonly number[]): number {
  let total = 0;
  for (let index = 1; index < layerWidths.length; index++) {
    const fanIn = layerWidths[index - 1] ?? 0;
    const fanOut = layerWidths[index] ?? 0;
    total += fanIn * fanOut + fanOut;
  }
  return total;
}

/** The count spelled out: `104×64 + 64 + 64×32 + 32 + 32×36 + 36`. */
export function parameterSum(layerWidths: readonly number[]): string {
  const terms: string[] = [];
  for (let index = 1; index < layerWidths.length; index++) {
    const fanIn = layerWidths[index - 1] ?? 0;
    const fanOut = layerWidths[index] ?? 0;
    terms.push(`${fanIn}×${fanOut} + ${fanOut}`);
  }
  return terms.join(" + ");
}

/**
 * A small deterministic generator (mulberry32) for the illustrative weights:
 * the same numbers on the server and in every browser, so nothing shifts on
 * hydration. These are pictures of weights, never an organism's genome.
 */
export function seeded(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
