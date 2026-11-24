// The v2 mechanisms' rules as plain functions, for the atlas's small charts
// (components/explain/v2-atlas.tsx). Each is the rule the engine implements,
// at the committed presets' numbers (presets/supercolony-v2.toml,
// presets/morphogenesis-tissue-v2.toml), and each cites its source; the tests
// (v2-atlas-math.test.ts) pin the values the page states in words.

/** The presets' tick: 1/32 simulated second ([world] timestep). */
export const DT = 1 / 32;

// ─── oases (V2-02; crates/flife-core/src/v2.rs, OasesRule) ───────────────────

export const OASES = { sources: 16, periodSeconds: 24, minimumFraction: 0.05 } as const;

/**
 * Source j's bloom weight at time t: w_j = f_min + (1 − f_min)·max(0, sin(2π(t/T + j/M)))².
 * The engine evaluates it at mid-tick, (n + ½)Δt; the chart samples t directly.
 */
export function oasisWeight(t: number, j: number, rule = OASES): number {
  const phase = 2 * Math.PI * (t / rule.periodSeconds + j / rule.sources);
  const lift = Math.max(0, Math.sin(phase));
  return rule.minimumFraction + (1 - rule.minimumFraction) * lift * lift;
}

/** Source j's power as a multiple of the preset's per-source power: P_j / P = M·w_j / Σw. The total stays M·P. */
export function oasisPowerShare(t: number, j: number, rule = OASES): number {
  let sum = 0;
  for (let k = 0; k < rule.sources; k++) sum += oasisWeight(t, k, rule);
  return (rule.sources * oasisWeight(t, j, rule)) / sum;
}

// ─── senescence (V2-03) ──────────────────────────────────────────────────────

export const SENESCENCE = { scaleSeconds: 30, maximumMultiplier: 64 } as const;

/** Basal maintenance multiplier at an age: 1 + min((age/τ)², M − 1). A cost, never a death draw. */
export function senescenceMultiplier(ageSeconds: number, rule = SENESCENCE): number {
  const ratio = Math.max(0, ageSeconds) / rule.scaleSeconds;
  return 1 + Math.min(ratio * ratio, rule.maximumMultiplier - 1);
}

/** The age at which the multiplier reaches its cap: τ·√(M − 1). */
export function senescenceCapAge(rule = SENESCENCE): number {
  return rule.scaleSeconds * Math.sqrt(rule.maximumMultiplier - 1);
}

// ─── surplus-emission (V2-04; crates/flife-engine/src/emission.rs) ──────────

export const SURPLUS_FLOOR = 0.45;

/** Energy emission may spend: max(E − floor, 0). */
export function emissionSurplus(energy: number, floor = SURPLUS_FLOOR): number {
  return Math.max(0, energy - floor);
}

// ─── excitable-relay (V2-04, vesicle-readiness-v1; emission.rs) ─────────────

export const EXCITABLE = { threshold: 0.5, depletion: 0.85, recoverySeconds: 1, initial: 0.2, burst: 5 } as const;

/**
 * Readiness over `ticks` ticks for an organism that always wants to emit and always has surplus:
 * at r ≥ threshold it fires a burst and keeps (1 − depletion)·r; otherwise r recovers by
 * (1 − r)·(1 − e^(−Δt/τ)) and nothing is emitted (emission.rs, `VesicleReadiness`).
 */
export function readinessTrace(ticks: number, rule = EXCITABLE, dt = DT): { readiness: number[]; fired: number[] } {
  const alpha = 1 - Math.exp(-dt / rule.recoverySeconds);
  const readiness: number[] = [];
  const fired: number[] = [];
  let r: number = rule.initial;
  for (let tick = 0; tick < ticks; tick++) {
    readiness.push(r);
    if (r >= rule.threshold) {
      fired.push(tick);
      r = (1 - rule.depletion) * r;
    } else {
      r = Math.min(1, r + (1 - r) * alpha);
    }
  }
  return { readiness, fired };
}

// ─── tight-junctions (V2-09; harmonic-v1) ────────────────────────────────────

export const BARRIER = { minimum: 0.04, impedance: 1.2 } as const;

/** A cell's permeability with n organisms in it: φ = φ_min + (1 − φ_min)/(1 + κn). A face takes the harmonic mean of its two cells'. */
export function permeability(occupants: number, rule = BARRIER): number {
  return rule.minimum + (1 - rule.minimum) / (1 + rule.impedance * Math.max(0, occupants));
}

// ─── vascular-field (V2-14; crates/flife-field/src/transport.rs, update_conductance) ──

export const VASCULAR = { minimum: 0.025, reinforce: 0.5, prune: 0.1, halfSaturation: 0.25, exponent: 1.5 } as const;

/** Saturation of a face's energy flux: s = r^μ/(1 + r^μ), r = |F|/J₀. */
export function fluxSaturation(flux: number, rule = VASCULAR): number {
  const ratio = Math.max(0, flux) / rule.halfSaturation;
  const raised = ratio ** rule.exponent;
  return raised / (1 + raised);
}

/** One conductance step: g + Δt(α·s·(1 − g) − γ(g − g_min)), clamped to [g_min, 1]. */
export function conductanceStep(g: number, flux: number, rule = VASCULAR, dt = DT): number {
  const next = g + dt * (rule.reinforce * fluxSaturation(flux, rule) * (1 - g) - rule.prune * (g - rule.minimum));
  return Math.min(1, Math.max(rule.minimum, next));
}

/** The conductance a face settles at under a steady flux: (α·s + γ·g_min)/(α·s + γ). */
export function steadyConductance(flux: number, rule = VASCULAR): number {
  const s = rule.reinforce * fluxSaturation(flux, rule);
  return (s + rule.prune * rule.minimum) / (s + rule.prune);
}

// ─── dual-carrier-trails (V2-15) and the food field's own decay ──────────────

/** The supercolony preset's [field] decay and [v2.carriers] trail decay, per simulated second. */
export const DECAY = { food: 0.0818013, trail: 0.03 } as const;

/** What is left of a deposit after t seconds of first-order decay (diffusion aside). */
export function remaining(t: number, perSecond: number): number {
  return Math.exp(-perSecond * Math.max(0, t));
}

/** Half-life of first-order decay: ln 2 / k. */
export function halfLife(perSecond: number): number {
  return Math.LN2 / perSecond;
}

// ─── hippo-crowding (V2-07; local-neighbors-v1) ──────────────────────────────

export const HIPPO = { arrest: 3.25 } as const;

/** The birth gate with N neighbours in reach: clamp(1 − N/c_max, 0, 1). */
export function hippoGate(neighbours: number, rule = HIPPO): number {
  return Math.min(1, Math.max(0, 1 - neighbours / rule.arrest));
}

// ─── epigenetic-recurrence (V2-12) ───────────────────────────────────────────

export const EPIGENETIC = { bias: 2.5 } as const;

/** The state's update gate: g = σ(z − bias). With no push (z = 0) it is about 0.076. */
export function epigeneticGate(z: number, rule = EPIGENETIC): number {
  return 1 / (1 + Math.exp(-(z - rule.bias)));
}

/** One state step: h' = (1 − g)·h + g·tanh(c). */
export function epigeneticStep(h: number, z: number, c: number, rule = EPIGENETIC): number {
  const g = epigeneticGate(z, rule);
  return (1 - g) * h + g * Math.tanh(c);
}

/** The angle, in radians, by which state h₀ ∈ [−1, 1] turns the diet: a_eff = cos(πh₀/2)·a + sin(πh₀/2)·Ja. */
export function affinityTurn(h0: number): number {
  return (Math.PI * Math.max(-1, Math.min(1, h0))) / 2;
}

// ─── trophallaxis (V2-13; crates/flife-engine/src/contact.rs, pair_trophallaxis_raw) ──

export const TROPHALLAXIS = { exponent: 4, threshold: 0.5, efficiency: 0.95 } as const;

/** How much kinship weighs a transfer: alike^p for alike = max(0, cos) at or above the threshold, else 0. */
export function kinWeight(cosine: number, rule = TROPHALLAXIS): number {
  const alike = Math.max(0, cosine);
  return alike < rule.threshold ? 0 : alike ** rule.exponent;
}

// ─── cadherin-4d (V2-13) ─────────────────────────────────────────────────────

/** Adhesion between two tags: min(s_i, s_j)·max(0, cos θ)². */
export function cadherinAdhesion(cosine: number, strengthI = 1, strengthJ = 1): number {
  const aligned = Math.max(0, cosine);
  return Math.min(strengthI, strengthJ) * aligned * aligned;
}
