import { describe, expect, test } from "bun:test";

import { END, INTERVAL, type Lanes, newestCheckpoint, toyDigest, worldAt } from "./branch-model";

const lanes: Lanes = { branchAt: 1024, brushAt: 2000, killAt: 2900 };
const digestOf = (lane: Parameters<typeof worldAt>[0], tick: number) => {
  const world = worldAt(lane, tick, lanes);
  return world === null ? null : toyDigest(world, tick);
};

describe("branch model", () => {
  test("a killed run resumes to the uninterrupted run's world at every tick", () => {
    for (let tick = 0; tick <= END; tick += 64) expect(digestOf("resumed", tick)).toBe(digestOf("run", tick));
  });

  test("a control branch has no world before its checkpoint and the run's world from it on", () => {
    expect(digestOf("control", lanes.branchAt - 1)).toBeNull();
    for (let tick = lanes.branchAt; tick <= END; tick += 64) expect(digestOf("control", tick)).toBe(digestOf("run", tick));
  });

  test("an intervention branch matches until its brush applies and differs after", () => {
    expect(digestOf("brushed", lanes.branchAt)).toBe(digestOf("run", lanes.branchAt));
    expect(digestOf("brushed", lanes.brushAt)).toBe(digestOf("run", lanes.brushAt));
    for (let tick = lanes.brushAt + 1; tick <= END; tick += 37) {
      expect(digestOf("brushed", tick)).not.toBe(digestOf("run", tick));
    }
  });

  test("checkpoints and digests are deterministic", () => {
    expect(newestCheckpoint(2900)).toBe(2560);
    expect(newestCheckpoint(INTERVAL)).toBe(INTERVAL);
    expect(toyDigest("run", 4096)).toBe(toyDigest("run", 4096));
    expect(toyDigest("run", 4096)).toMatch(/^[0-9a-f]{8}$/);
  });
});
