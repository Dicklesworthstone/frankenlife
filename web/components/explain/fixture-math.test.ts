// `bun test`: the fixture bench's arithmetic reproduces Appendix C's stated
// answers, and its wrong versions give the wrong answers the plan names.
import { describe, expect, test } from "bun:test";

import {
  C9_CHEMISTRY,
  FIXTURES,
  birthChargeFirst,
  birthRule,
  decodeLinear,
  decodeTanhFirst,
  minimumImage,
  minimumImageBetween,
  mix,
  packet,
  settle,
  settleMixture,
  settlePackets,
  settleRequested,
  signedFood,
  threadOrder,
  unambiguous,
  uptakeInOrder,
  uptakeRacing,
  uptakeRule,
  usefulGains,
} from "./fixture-math";

describe("C.1 contended uptake", () => {
  const { cell, requests, efficiencies } = FIXTURES.c1;

  test("grants 4 and 6, gains 2 and 1.5, dissipation 6.5", () => {
    const uptake = uptakeRule(cell, requests);
    expect(uptake.grants.map((grant) => grant.granted)).toEqual([...FIXTURES.c1.grants]);
    expect(uptake.remaining).toBe(0);
    const { gains, dissipated } = usefulGains(
      uptake.grants.map((grant) => grant.granted),
      efficiencies,
    );
    expect(gains).toEqual([...FIXTURES.c1.gains]);
    expect(dissipated).toBe(FIXTURES.c1.dissipated);
  });

  test("the rule ignores thread order; one pass in order does not", () => {
    expect(uptakeInOrder(cell, requests, [0, 1]).grants.map((grant) => grant.granted)).toEqual([8, 2]);
    expect(uptakeInOrder(cell, requests, [1, 0]).grants.map((grant) => grant.granted)).toEqual([0, 10]);
  });

  test("racing threads create energy that clamping the cell hides", () => {
    const racing = uptakeRacing(cell, requests);
    expect(racing.grants.map((grant) => grant.granted)).toEqual([8, 10]);
    expect(racing.created).toBe(8);
    expect(racing.remaining).toBe(0);
  });

  test("leftover quanta go to the largest remainders, ties to the lower identity", () => {
    expect(uptakeRule(10, [5, 5, 5]).grants.map((grant) => grant.granted)).toEqual([4, 3, 3]);
    // Grants never exceed the cell and always sum to it when contended.
    for (let a = 0; a <= 16; a++) {
      for (let b = 0; b <= 16; b++) {
        const uptake = uptakeRule(10, [a, b, 7]);
        const total = uptake.grants.reduce((sum, grant) => sum + grant.granted, 0);
        expect(total).toBeLessThanOrEqual(10);
        if (a + b + 7 > 10) expect(total).toBe(10);
        for (const grant of uptake.grants) expect(Math.abs(grant.granted - grant.ideal)).toBeLessThan(1);
      }
    }
  });

  test("thread orders are permutations and the first is identity order", () => {
    expect(threadOrder(3, 0)).toEqual([0, 1, 2]);
    for (let index = 1; index < 20; index++) expect([...threadOrder(4, index)].sort()).toEqual([0, 1, 2, 3]);
  });
});

describe("C.1 with no demand", () => {
  test("an empty cell or an empty request grants nothing, never NaN", () => {
    expect(uptakeRule(0, [0]).grants.map((g) => g.granted)).toEqual([0]);
    expect(uptakeRule(5, [0, 0]).grants.map((g) => g.granted)).toEqual([0, 0]);
    expect(uptakeRule(0, [3, 0]).grants.map((g) => g.granted)).toEqual([0, 0]);
  });
});

describe("C.1 exact ties", () => {
  test("equal remainders go to the lower identity, compared exactly", () => {
    // e = 7, requests 10, 4, 7 (A = 21): ideals 10/3, 4/3, 7/3, every remainder 1/3. Floors 3, 1, 2 leave one
    // quantum, which goes to identity 0. In floating point 28/21 - 1 > 70/21 - 3, which handed it to identity 1.
    expect(uptakeRule(7, [10, 4, 7]).grants.map((grant) => grant.granted)).toEqual([4, 1, 2]);
  });
});

describe("C.3 opposing signatures", () => {
  test("(3, 3a) + (5, −5a) = (8, −2a); the energy has not become 2", () => {
    const cell = mix([packet(3, 0), packet(5, Math.PI)]);
    expect(cell.e).toBe(FIXTURES.c3.energy);
    expect(cell.m[0]).toBeCloseTo(FIXTURES.c3.moment, 12);
    expect(Math.hypot(...cell.m)).toBeLessThanOrEqual(cell.e);
    expect(signedFood([packet(3, 0), packet(5, Math.PI)])).toBeCloseTo(2, 12);
  });
});

describe("C.2 toxicity clipping", () => {
  const { energy, absorbed, gain, requestedToxicity } = FIXTURES.c2;

  test("actual toxicity 2, dissipation 5", () => {
    const rule = settle(absorbed, gain, requestedToxicity, energy);
    expect(rule.toxicity).toBe(FIXTURES.c2.toxicity);
    expect(rule.dissipated).toBe(FIXTURES.c2.dissipated);
    expect(energy + rule.net).toBe(0);
  });

  test("debiting the requested toxicity records 8 and leaves −3", () => {
    const wrong = settleRequested(absorbed, gain, requestedToxicity);
    expect(wrong.dissipated).toBe(FIXTURES.c2.wrongRecorded);
    expect(energy + wrong.net).toBe(-3);
  });
});

describe("C.9 packet chemistry is not mixture chemistry", () => {
  const packets = [
    { u: 1, dot: 1 },
    { u: 1, dot: -1 },
  ];

  test("per packet ≈ +0.639355310069", () => {
    expect(Math.abs(settlePackets(packets, 10, C9_CHEMISTRY).net - FIXTURES.c9.perPacket)).toBeLessThan(1e-11);
  });

  test("on the mixture ≈ −0.047643210759", () => {
    expect(Math.abs(settleMixture(packets, 10, C9_CHEMISTRY).net - FIXTURES.c9.mixture)).toBeLessThan(1e-11);
  });
});

describe("C.11 output logits", () => {
  test("sigmoid once: 0.99966465; tanh first: 0.73105853", () => {
    expect(Math.abs(decodeLinear(FIXTURES.c11.logit) - FIXTURES.c11.linear)).toBeLessThan(5e-9);
    expect(Math.abs(decodeTanhFirst(FIXTURES.c11.logit) - FIXTURES.c11.tanhFirst)).toBeLessThan(5e-9);
  });
});

describe("C.15 skipped epochs on a torus", () => {
  const { width, from, moved } = FIXTURES.c15;

  test("80 forward is drawn 48 in reverse by the minimum image", () => {
    expect(minimumImage(from, from + moved, width)).toBe(-FIXTURES.c15.reverse);
    expect(minimumImageBetween(from, from + moved, width, 0.5)).toBe(FIXTURES.c15.halfwayWrong);
    expect(unambiguous(moved, width)).toBe(false);
  });

  test("the same cases as flife-render's interpolation test", () => {
    expect(minimumImageBetween(10, 90, 128, 0)).toBe(10);
    expect(minimumImageBetween(10, 90, 128, 1)).toBe(90);
    expect(minimumImageBetween(120, 8, 128, 0.5)).toBe(0);
    expect(minimumImageBetween(8, 120, 128, 0.25)).toBe(4);
  });
});

describe("C.5 birth", () => {
  const { parent, cost, share } = FIXTURES.c5;

  test("child 3, parent 6, dissipation 1", () => {
    const birth = birthRule(parent, cost, share, true);
    expect(birth.child).toBeCloseTo(FIXTURES.c5.child, 12);
    expect(birth.parent).toBeCloseTo(FIXTURES.c5.kept, 12);
    expect(birth.dissipated).toBe(FIXTURES.c5.dissipated);
  });

  test("a refused birth charges nothing; charging first loses the endowment", () => {
    expect(birthRule(parent, cost, share, false)).toEqual({ parent, child: null, dissipated: 0, vanished: 0 });
    const wrong = birthChargeFirst(parent, cost, share, false);
    expect(wrong.child).toBeNull();
    expect(wrong.vanished).toBeCloseTo(3, 12);
  });
});
