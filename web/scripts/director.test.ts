// `bun test`: the director's camera path on synthetic frames whose activity
// is known.
import { describe, expect, test } from "bun:test";

import { activities, direct } from "./director";

const GRID = 128;
const EXTENT: [number, number] = [256, 256];

/** A frame of GRID² cells with a bright disc of radius 4 cells at `center` (cells), on a dim floor. */
function disc(center: [number, number]): Float64Array {
  const frame = new Float64Array(GRID * GRID).fill(1);
  for (let y = 0; y < GRID; y++) {
    for (let x = 0; x < GRID; x++) {
      if ((x - center[0]) ** 2 + (y - center[1]) ** 2 <= 16) frame[y * GRID + x] = 200;
    }
  }
  return frame;
}

/** Torus distance in world units. */
function distance(a: [number, number], b: [number, number]): number {
  let total = 0;
  for (const axis of [0, 1] as const) {
    const d = Math.abs(a[axis] - b[axis]) % EXTENT[axis];
    total += Math.min(d, EXTENT[axis] - d) ** 2;
  }
  return Math.sqrt(total);
}

const options = { opening: 10, transition: 10, dwell: 30, closing: 10, zooms: [4] };

describe("direct", () => {
  test("opens and closes on the whole torus, with strictly increasing ticks", () => {
    const frames = Array.from({ length: 200 }, (_, t) => disc([20 + (t % 2), 30]));
    const ticks = frames.map((_, t) => 1000 + 8 * t);
    const keys = direct(ticks, activities(frames), EXTENT, options);
    expect(keys[0]).toEqual({ tick: 1000, center: [128, 128], zoom: 1 });
    expect(keys.at(-1)).toEqual({ tick: 1000 + 8 * 199, center: [128, 128], zoom: 1 });
    for (let i = 1; i < keys.length; i++) expect(keys[i]!.tick).toBeGreaterThan(keys[i - 1]!.tick);
    expect(keys.some((key) => key.zoom === 4)).toBe(true);
  });

  test("looks at the moving disc, not at the still floor", () => {
    // The disc crosses the world left to right, a cell a frame, at row 64.
    const frames = Array.from({ length: 200 }, (_, t) => disc([(10 + t) % GRID, 64]));
    const ticks = frames.map((_, t) => t);
    const keys = direct(ticks, activities(frames), EXTENT, options);
    const shots = keys.filter((key) => key.zoom === 4);
    expect(shots.length).toBeGreaterThanOrEqual(4);
    for (const key of shots) {
      // Where the disc is at the key's frame, in world units (2 per cell).
      const at: [number, number] = [(((10 + key.tick) % GRID) + 0.5) * 2, 64.5 * 2];
      // Within a quarter of the view (256 / 4 / 4 = 16 units) of it, plus the drift's lead.
      expect(distance(key.center, at)).toBeLessThan(48);
      expect(Math.abs(key.center[1] - 129)).toBeLessThan(4);
    }
  });

  test("the region just shown gives way to another as active", () => {
    // Two discs move alike, a cell a frame, at opposite corners: consecutive
    // shots alternate between them. (A disc that only flickers with period 2
    // has no central difference, so it would show no activity at all.)
    const frames = Array.from({ length: 200 }, (_, t) => {
      const frame = new Float64Array(GRID * GRID).fill(1);
      const offset = Math.abs((t % 16) - 8);
      for (const [cx, cy] of [
        [28, 32],
        [92, 96],
      ] as const) {
        for (let y = cy - 3; y <= cy + 3; y++) {
          for (let x = cx + offset - 3; x <= cx + offset + 3; x++) frame[y * GRID + x] = 200;
        }
      }
      return frame;
    });
    const keys = direct(
      frames.map((_, t) => t),
      activities(frames),
      EXTENT,
      { ...options, dwell: 20 },
    );
    const targets = keys.filter((key) => key.zoom === 4).map((key) => key.center);
    // Each shot's first key: every other key of a shot is its drift.
    const firsts = targets.filter((_, index) => index % 2 === 0);
    expect(firsts.length).toBeGreaterThanOrEqual(3);
    for (let i = 1; i < firsts.length; i++) {
      expect(distance(firsts[i]!, firsts[i - 1]!)).toBeGreaterThan(64);
    }
  });
});
