import { describe, expect, test } from "bun:test";

import { uptakeRule } from "@/components/explain/fixture-math";
import {
  CAPACITY,
  type Mode,
  PARAMETERS,
  RULES,
  World,
  grantQuanta,
  random,
  replay,
  sinToy,
  tanhToy,
} from "@/components/explain/miniature-engine";

const OWN: Mode = { brains: "own", children: "inherit" };
const SHARED: Mode = { brains: "shared", children: "inherit" };
const FRESH: Mode = { brains: "own", children: "fresh" };

/** The largest weight difference between two organisms, by slot. */
function weightGap(world: World, a: number, b: number): number {
  let gap = 0;
  for (let p = 0; p < PARAMETERS; p++) {
    gap = Math.max(gap, Math.abs((world.weights[a * PARAMETERS + p] ?? 0) - (world.weights[b * PARAMETERS + p] ?? 0)));
  }
  return gap;
}

describe("uptake on whole quanta", () => {
  test("fixture C.1: e = 10, requests 8 and 12 → grants 4 and 6", () => {
    const grants = [0, 0];
    expect(grantQuanta(10, [8, 12], 2, grants)).toBe(10);
    expect(grants).toEqual([4, 6]);
  });

  test("uncontended: everyone gets what they asked", () => {
    const grants = [0, 0, 0];
    expect(grantQuanta(50, [3, 7, 10], 3, grants)).toBe(20);
    expect(grants).toEqual([3, 7, 10]);
  });

  /** The rule in exact integers: ⌊q·e/A⌋ each, leftovers by remainder q·e mod A, ties to the lower identity. */
  function exact(cell: number, requests: number[]): { grants: number[]; tied: boolean } {
    const demand = requests.reduce((sum, q) => sum + q, 0);
    if (demand <= cell) return { grants: [...requests], tied: false };
    const grants = requests.map((q) => Math.floor((q * cell) / demand));
    const rests = requests.map((q, id) => ({ id, rest: (q * cell) % demand }));
    let left = cell - grants.reduce((sum, g) => sum + g, 0);
    for (const { id } of [...rests].sort((a, b) => b.rest - a.rest || a.id - b.id)) {
      if (left === 0) break;
      grants[id] = (grants[id] ?? 0) + 1;
      left -= 1;
    }
    const tied = new Set(rests.map((r) => r.rest)).size < rests.length;
    return { grants, tied };
  }

  test("is the exact rule on random integer cells, and the page's C.1 rule on every one, ties included", () => {
    const next = random(7);
    let tiedCases = 0;
    for (let trial = 0; trial < 4000; trial++) {
      const n = 1 + Math.floor(next() * 6);
      const requests = Array.from({ length: n }, () => Math.floor(next() * 11));
      const cell = Math.floor(next() * 40);
      const grants = new Array<number>(n).fill(0);
      const granted = grantQuanta(cell, requests, n, grants);
      const reference = exact(cell, requests);
      expect(grants).toEqual(reference.grants);
      expect(granted).toBe(reference.grants.reduce((sum, g) => sum + g, 0));
      expect(granted).toBeLessThanOrEqual(cell);
      // fixture-math computes whole-number remainders exactly, so equal remainders tie exactly there too.
      expect(grants).toEqual(uptakeRule(cell, requests, 1).grants.map((grant) => grant.granted));
      if (reference.tied) tiedCases += 1;
    }
    // The sweep must actually exercise ties, or "ties included" would be vacuous.
    expect(tiedCases).toBeGreaterThan(100);
  });

  test("an exact three-way tie goes to the lowest identity", () => {
    // e = 7, requests 10, 4, 7: A = 21, every remainder is 7/21, one quantum left.
    const grants = [0, 0, 0];
    grantQuanta(7, [10, 4, 7], 3, grants);
    expect(grants).toEqual([4, 1, 2]);
  });
});

describe("arithmetic the same in every engine", () => {
  test("tanhToy is odd, monotone, within 0.03 of tanh and exactly ±1 beyond ±3", () => {
    let last = -2;
    for (let x = -5; x <= 5; x += 0.01) {
      const y = tanhToy(x);
      expect(y).toBeGreaterThanOrEqual(last);
      expect(Math.abs(y - Math.tanh(x))).toBeLessThan(0.03);
      expect(tanhToy(-x)).toBeCloseTo(-y, 12);
      last = y;
    }
    expect(tanhToy(3)).toBe(1);
    expect(tanhToy(-7)).toBe(-1);
  });

  test("sinToy is within 1e-5 of sin", () => {
    for (let x = -20; x <= 20; x += 0.013) expect(Math.abs(sinToy(x) - Math.sin(x))).toBeLessThan(1e-5);
  });
});

describe("the ledger", () => {
  for (const [name, mode] of [
    ["own", OWN],
    ["shared", SHARED],
    ["fresh", FRESH],
  ] as const) {
    test(`injected = field + bodies + dissipated on every tick (${name})`, () => {
      for (const seed of [1, 2, 3]) {
        const world = new World(seed, mode);
        for (let t = 0; t < 2500; t++) {
          if (t === 700) world.brush(30.5, 40.2);
          world.step();
          const ledger = world.ledger();
          expect(ledger.residual).toBe(0);
          expect(ledger.field).toBeGreaterThanOrEqual(0);
          expect(world.count).toBeLessThanOrEqual(CAPACITY);
        }
        for (let c = 0; c < world.field.length; c++) expect(world.field[c] ?? 0).toBeGreaterThanOrEqual(0);
        for (let i = 0; i < world.count; i++) expect(world.energy[i] ?? 0).toBeGreaterThanOrEqual(0);
      }
    });
  }

  test("a long run stays exact and alive", () => {
    const world = new World(4, OWN);
    for (let t = 0; t < 12000; t++) world.step();
    expect(world.ledger().residual).toBe(0);
    expect(world.count).toBeGreaterThan(200);
  });
});

describe("determinism", () => {
  test("the same seed reaches the same digest; another seed does not", () => {
    const a = new World(11, OWN);
    const b = new World(11, OWN);
    const c = new World(12, OWN);
    for (let t = 0; t < 1500; t++) {
      a.step();
      b.step();
      c.step();
      if (t % 250 === 0) expect(a.digest()).toBe(b.digest());
    }
    expect(a.digest()).toBe(b.digest());
    expect(a.digest()).not.toBe(c.digest());
  });

  test("replay from the seed, with the brushes, reaches the live world's digest", () => {
    const live = new World(5, OWN);
    for (let t = 0; t < 1800; t++) {
      if (t === 300) live.brush(10, 10);
      if (t === 900) live.brush(60.7, 22.1, 900);
      live.step();
    }
    const again = replay(5, OWN, live.commands, live.tick);
    expect(again.tick).toBe(live.tick);
    expect(again.digest()).toBe(live.digest());
    expect(again.ledger()).toEqual(live.ledger());
  });

  test("a brush branches the world from its tick on, and only from then", () => {
    const control = new World(6, OWN);
    const brushed = new World(6, OWN);
    for (let t = 0; t < 400; t++) {
      if (t === 200) {
        expect(brushed.digest()).toBe(control.digest());
        brushed.brush(48, 48);
      }
      control.step();
      brushed.step();
    }
    expect(brushed.digest()).not.toBe(control.digest());
    expect(brushed.ledger().injected - control.ledger().injected).toBe(RULES.brush);
  });

  test("a switch is part of the world: the same seed under another mode differs", () => {
    const own = replay(8, OWN, [], 600);
    const shared = replay(8, SHARED, [], 600);
    expect(own.digest()).not.toBe(shared.digest());
  });
});

describe("brains and inheritance", () => {
  test("each its own: no two founders share their weights", () => {
    const world = new World(9, OWN);
    for (let i = 1; i < world.count; i++) expect(weightGap(world, 0, i)).toBeGreaterThan(0);
  });

  test("one shared brain: every organism runs the same weights, mutation does nothing", () => {
    const world = new World(9, SHARED);
    for (let t = 0; t < 1200; t++) world.step();
    expect(world.births).toBeGreaterThan(0);
    for (let i = 1; i < world.count; i++) expect(weightGap(world, 0, i)).toBe(0);
  });

  /** For each birth on ticks [from, to): the largest weight change from parent to child. */
  function childGaps(mode: Mode, from: number, to: number): number[] {
    const world = new World(10, mode);
    const gaps: number[] = [];
    while (world.tick < to) {
      world.step();
      if (world.tick < from) continue;
      for (let k = 0; k < world.lastBirths.length; k += 2) {
        const parent = world.find(world.lastBirths[k] ?? -1);
        const child = world.find(world.lastBirths[k + 1] ?? -1);
        if (parent >= 0 && child >= 0) gaps.push(weightGap(world, parent, child));
      }
    }
    return gaps;
  }

  // A unit draw is bounded by 2√3, so an inherited weight moves by at most mutation · 2√3.
  const bound = RULES.mutation * 2 * Math.sqrt(3);

  test("children inherit: a mutated copy of the parent's weights", () => {
    const gaps = childGaps(OWN, 400, 600);
    expect(gaps.length).toBeGreaterThan(50);
    for (const gap of gaps) {
      expect(gap).toBeGreaterThan(0);
      expect(gap).toBeLessThanOrEqual(bound + 1e-12);
    }
  });

  test("fresh offspring: children's weights are drawn anew, not copied", () => {
    const gaps = childGaps(FRESH, 400, 600);
    expect(gaps.length).toBeGreaterThan(50);
    const far = gaps.filter((gap) => gap > bound).length;
    expect(far / gaps.length).toBeGreaterThan(0.9);
  });

  test("no cell holds more bodies than it has room for, once the founders are gone", () => {
    const world = new World(13, OWN);
    while (world.tick < RULES.maxAge + 200) world.step();
    const occupancy = new Map<number, number>();
    for (let i = 0; i < world.count; i++) {
      const cell = World.cell(world.x[i] ?? 0, world.y[i] ?? 0);
      occupancy.set(cell, (occupancy.get(cell) ?? 0) + 1);
    }
    expect(Math.max(...occupancy.values())).toBeLessThanOrEqual(RULES.perCell);
  });

  test("identities stay in order and lineages are kept by children", () => {
    const world = new World(12, OWN);
    for (let t = 0; t < 800; t++) {
      world.step();
      for (let k = 0; k < world.lastBirths.length; k += 2) {
        const parent = world.find(world.lastBirths[k] ?? -1);
        const child = world.find(world.lastBirths[k + 1] ?? -1);
        if (parent >= 0 && child >= 0) expect(world.lineage[child]).toBe(world.lineage[parent] ?? -1);
      }
    }
    for (let i = 1; i < world.count; i++) expect(world.identity[i] ?? 0).toBeGreaterThan(world.identity[i - 1] ?? 0);
  });
});
