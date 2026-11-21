// The explainers' arithmetic against the figures the repository states: plan
// §6.4's catalog table, README.md's "Why the scale is hard" table and plan
// §19.5's energy budget. Expected values are copied from those documents, not
// computed with the helpers under test.

import { describe, expect, test } from "bun:test";

import { ACTION_WIDTH, CATALOG, INPUT_GROUPS, OUTPUT_GROUPS, SENSOR_WIDTH, parameterCount, parameterSum, seeded, widths } from "./brain-math";
import { count, gibibytes, mandatoryCost, rate, scale, sustainable } from "./scale-math";

describe("controller shape", () => {
  test("the groups fill the 104 inputs and 36 outputs", () => {
    expect(INPUT_GROUPS.reduce((sum, group) => sum + group.size, 0)).toBe(SENSOR_WIDTH);
    expect(OUTPUT_GROUPS.reduce((sum, group) => sum + group.size, 0)).toBe(ACTION_WIDTH);
    expect(SENSOR_WIDTH).toBe(104);
    expect(ACTION_WIDTH).toBe(36);
  });

  test("the catalog's parameter counts are plan §6.4's table", () => {
    expect(CATALOG.map((topology) => parameterCount(widths(topology)))).toEqual([1_164, 4_548, 9_988, 18_628, 29_316]);
  });

  test("the default network's count spelled out", () => {
    expect(parameterSum([104, 64, 32, 36])).toBe("104×64 + 64 + 64×32 + 32 + 32×36 + 36");
  });

  test("the illustrative weights are the same on every run", () => {
    const first = seeded(7);
    const second = seeded(7);
    const a = [first(), first(), first()];
    expect([second(), second(), second()]).toEqual(a);
    for (const value of a) {
      expect(value).toBeGreaterThanOrEqual(0);
      expect(value).toBeLessThan(1);
    }
  });
});

describe("README's scale table (400,000 organisms × 9,988 parameters)", () => {
  // README rounds the bank to 4.0 billion parameters; the exact product is 3,995,200,000, 0.12% less.
  const rows: [number, number, number, number][] = [
    // sweeps/s, multiply-accumulates/s, F16 bytes/s, F32 bytes/s
    [15, 60e9, 120e9, 240e9],
    [30, 120e9, 240e9, 480e9],
    [60, 240e9, 480e9, 960e9],
    [128, 512e9, 1.024e12, 2.048e12],
  ];
  for (const [sweeps, macs, f16, f32] of rows) {
    test(`${sweeps} sweeps a second`, () => {
      const half = scale(400_000, 9_988, sweeps, "f16");
      const single = scale(400_000, 9_988, sweeps, "f32");
      expect(Math.abs(half.macsPerSecond / macs - 1)).toBeLessThan(0.002);
      expect(Math.abs(half.streamBytesPerSecond / f16 - 1)).toBeLessThan(0.002);
      expect(Math.abs(single.streamBytesPerSecond / f32 - 1)).toBeLessThan(0.002);
    });
  }

  test("the bank: about four billion parameters, 7.45 GiB in F16 and 14.90 GiB in F32 (rounded)", () => {
    const half = scale(400_000, 9_988, 30, "f16");
    expect(half.parameters).toBe(3_995_200_000);
    expect(count(half.parameters)).toBe("3.995 billion");
    expect(Math.abs(half.bankBytes / 2 ** 30 - 7.45)).toBeLessThan(0.01);
    expect(Math.abs(scale(400_000, 9_988, 30, "f32").bankBytes / 2 ** 30 - 14.9)).toBeLessThan(0.02);
    expect(gibibytes(half.bankBytes)).toBe("7.44 GiB");
  });

  test("rates read as the table writes them", () => {
    expect(rate(scale(400_000, 9_988, 30, "f16").streamBytesPerSecond)).toBe("240 GB/s");
    expect(rate(scale(400_000, 9_988, 128, "f32").streamBytesPerSecond)).toBe("2.05 TB/s");
  });
});

describe("plan §19.5's energy budget", () => {
  test("a default organism's mandatory cost and 400,000 of them", () => {
    expect(mandatoryCost(9_988)).toBeCloseTo(0.008556928, 12);
    expect(400_000 * mandatoryCost(9_988)).toBeCloseTo(3_422.7712, 6);
  });

  test("the starter's 640 energy/s sustains at most about 74,793", () => {
    expect(Math.floor(sustainable(9_988))).toBe(74_793);
  });
});
