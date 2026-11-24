// Pins every value the atlas states in words to the rule that produces it.

import { describe, expect, test } from "bun:test";

import {
  affinityTurn,
  cadherinAdhesion,
  conductanceStep,
  DECAY,
  emissionSurplus,
  epigeneticGate,
  halfLife,
  hippoGate,
  kinWeight,
  OASES,
  oasisPowerShare,
  oasisWeight,
  permeability,
  readinessTrace,
  senescenceCapAge,
  senescenceMultiplier,
  steadyConductance,
} from "./v2-atlas-math";

describe("oases", () => {
  test("total power is constant while each source blooms between 5% and 100% of its weight", () => {
    for (let t = 0; t < 24; t += 0.37) {
      let total = 0;
      for (let j = 0; j < OASES.sources; j++) total += oasisPowerShare(t, j);
      expect(total).toBeCloseTo(OASES.sources, 10);
    }
    expect(oasisWeight(6, 0)).toBeCloseTo(1, 12); // sin(π/2)² at a quarter period
    expect(oasisWeight(18, 0)).toBeCloseTo(0.05, 12); // the trough holds the floor
  });
});

describe("senescence", () => {
  test("doubles maintenance at 30 s, fivefold at 60 s, caps at 64× from about 238 s", () => {
    expect(senescenceMultiplier(0)).toBe(1);
    expect(senescenceMultiplier(30)).toBeCloseTo(2, 12);
    expect(senescenceMultiplier(60)).toBeCloseTo(5, 12);
    expect(senescenceCapAge()).toBeCloseTo(238.1, 1);
    expect(senescenceMultiplier(1000)).toBe(64);
  });
});

describe("surplus-emission", () => {
  test("spends nothing at or below 0.45", () => {
    expect(emissionSurplus(0.3)).toBe(0);
    expect(emissionSurplus(0.45)).toBe(0);
    expect(emissionSurplus(1.45)).toBeCloseTo(1, 12);
  });
});

describe("excitable-relay", () => {
  test("an always-willing emitter pulses every 21 ticks, about two-thirds of a simulated second", () => {
    const { readiness, fired } = readinessTrace(32 * 8);
    expect(fired.length).toBeGreaterThan(10);
    const gaps = fired.slice(1).map((tick, at) => tick - (fired[at] ?? 0));
    for (const gap of gaps.slice(1)) expect(gap).toBe(gaps[1] ?? -1);
    expect(gaps[1]).toBe(21);
    expect(Math.max(...readiness)).toBeLessThan(0.53);
  });
});

describe("tight-junctions", () => {
  test("one occupant lets about 48% through, two about 32%, never below 4%", () => {
    expect(permeability(0)).toBe(1);
    expect(permeability(1)).toBeCloseTo(0.476, 3);
    expect(permeability(2)).toBeCloseTo(0.322, 3);
    expect(permeability(1e9)).toBeGreaterThan(0.04);
  });
});

describe("vascular-field", () => {
  test("faces settle near 0.84 under strong flux, 0.72 at half-saturation, and prune to 0.025 with none", () => {
    expect(steadyConductance(1e6)).toBeCloseTo(0.8375, 3);
    expect(steadyConductance(0.25)).toBeCloseTo(0.7214, 3);
    expect(steadyConductance(0)).toBeCloseTo(0.025, 12);
    // Iterating the per-tick update reaches the closed-form steady state.
    let g = 0.025;
    for (let tick = 0; tick < 32 * 200; tick++) g = conductanceStep(g, 0.25);
    expect(g).toBeCloseTo(steadyConductance(0.25), 6);
  });
});

describe("dual-carrier-trails", () => {
  test("trail energy outlives food: half-lives of about 23 s against 8.5 s", () => {
    expect(halfLife(DECAY.trail)).toBeCloseTo(23.1, 1);
    expect(halfLife(DECAY.food)).toBeCloseTo(8.47, 2);
  });
});

describe("hippo-crowding", () => {
  test("gates birth to 1, 0.69, 0.38, 0.08 and 0 with 0–4 neighbours", () => {
    expect([0, 1, 2, 3, 4].map((n) => Math.round(hippoGate(n) * 100) / 100)).toEqual([1, 0.69, 0.38, 0.08, 0]);
  });
});

describe("epigenetic-recurrence", () => {
  test("the resting gate is about 0.076 and h₀ = 1 turns the diet a right angle", () => {
    expect(epigeneticGate(0)).toBeCloseTo(0.0759, 4);
    expect(affinityTurn(1)).toBeCloseTo(Math.PI / 2, 12);
    expect(affinityTurn(-0.5)).toBeCloseTo(-Math.PI / 4, 12);
  });
});

describe("trophallaxis and cadherin-4d", () => {
  test("kin weight is cos⁴ from cos 0.5 up, nothing below; adhesion is cos² of the tags", () => {
    expect(kinWeight(0.49)).toBe(0);
    expect(kinWeight(0.5)).toBeCloseTo(0.0625, 12);
    expect(kinWeight(1)).toBe(1);
    expect(cadherinAdhesion(Math.cos(Math.PI / 3))).toBeCloseTo(0.25, 12);
    expect(cadherinAdhesion(-1)).toBe(0);
  });
});
