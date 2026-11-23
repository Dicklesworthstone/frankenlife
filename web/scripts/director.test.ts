// `bun test`: the director's camera path on synthetic frames whose activity
// is known.
import { describe, expect, test } from "bun:test";

import { type CameraKey, activities, cardFrame, direct } from "./director";

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
  test("801-frame true-60fps cadence (ticks 2,400..5,600, every 4) keeps exact 3-shot wall-clock parity with 401f/30fps", () => {
    const frameCount = 801;
    const ticks = Array.from({ length: frameCount }, (_, f) => 2400 + 4 * f);
    const frames = Array.from({ length: frameCount }, (_, t) => disc([(10 + Math.floor(t / 2)) % GRID, 64]));
    const opts60 = {
      opening: Math.round(frameCount * 0.08), // 64 frames = 1.07s at 60fps
      transition: Math.round(frameCount * 0.07), // 56 frames = 0.93s
      dwell: Math.round(frameCount * 0.16), // 128 frames = 2.13s
      closing: Math.round(frameCount * 0.06), // 48 frames = 0.80s
      zooms: [1.6, 3, 2.2, 5, 2.6],
    };
    const keys = direct(ticks, activities(frames), [80, 80], opts60);
    // 10 keys: open(0..64), shot0(120..248), shot1(304..432), shot2(488..616), close(672..800).
    expect(keys).toHaveLength(10);
    expect(keys.map((k) => k.tick)).toEqual([2400, 2656, 2880, 3392, 3616, 4128, 4352, 4864, 5088, 5600]);
    expect(keys.map((k) => k.zoom)).toEqual([1, 1, 1.6, 1.6, 3, 3, 2.2, 2.2, 1, 1]);
    // Closest shot is zoom=3 across ticks 3616..4128 -> middle tick 3872 is frame 368 (exact match to 30fps frame 184!).
    expect(cardFrame(keys, ticks)).toBe(368);
    expect(ticks[cardFrame(keys, ticks)]).toBe(3872);
  });
});

describe("cardFrame", () => {
  const whole: [number, number] = [128, 128];
  const key = (tick: number, zoom: number): CameraKey => ({ tick, center: whole, zoom });
  const ticks = Array.from({ length: 41 }, (_, frame) => 2400 + 8 * frame);

  test("is the middle frame of the closest shot", () => {
    const keys = [key(2400, 1), key(2480, 1), key(2496, 1.6), key(2560, 1.6), key(2576, 3), key(2656, 3), key(2720, 1)];
    // The zoom-3 shot holds from 2,576 to 2,656: its middle, 2,616, is frame 27.
    expect(cardFrame(keys, ticks)).toBe(27);
  });

  test("takes the first of two shots at the highest zoom", () => {
    const keys = [key(2400, 1), key(2480, 3), key(2560, 3), key(2576, 2), key(2640, 2), key(2656, 3), key(2720, 3)];
    expect(cardFrame(keys, ticks)).toBe(15);
  });

  test("is the first frame for a path that never zooms in", () => {
    expect(cardFrame([key(2400, 1), key(2720, 1)], ticks)).toBe(0);
    expect(cardFrame([], ticks)).toBe(0);
  });
});
