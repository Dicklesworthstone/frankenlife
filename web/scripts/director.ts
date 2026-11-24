// The film's camera: where to look, chosen from a whole-world preview of the
// same frames (`flife render --camera`'s path format; flife-cli's camera.rs).
//
// Presentation only. The preview is the same run rendered small, as raw dots
// without the field; the director reads its pixels and nothing else, and
// the path it writes changes no result of the render that follows it.
//
// The rule, stated so the film can say how its shots were chosen:
// 1. Each preview frame becomes a grid of mean luminance, `GRID` cells a side.
// 2. A frame's activity is |blur(L[t+1] - L[t-1])|: the change in where the
//    organisms are, blurred before the absolute value so that particles
//    jittering in place cancel and moving structure (fronts, streams,
//    colonies growing or dying) remains.
// 3. The film opens on the whole torus, then cuts between shots: each eases
//    in to the point whose surroundings, the size of the shot's view, saw the
//    most activity over the shot, drifts towards where that activity is in
//    the shot's second half, and the film ends on the whole torus again.
//    A region a shot has shown is discounted for the shots after it, the
//    most recent most.

import { spawnSync } from "node:child_process";

/** One key of a camera path, as `flife render --camera` reads it. */
export type CameraKey = { tick: number; center: [number, number]; zoom: number };

export type DirectorOptions = {
  /** Frames the film opens on the whole torus for. */
  opening: number;
  /** Frames easing from one view to the next. */
  transition: number;
  /** Frames each shot holds (drifting) after its transition. */
  dwell: number;
  /** Frames the film closes on the whole torus for. */
  closing: number;
  /** Zooms of the shots in turn, cycled. */
  zooms: number[];
};

export const DIRECTOR_RULE =
  "activity |blur(L[t+1] − L[t−1])| of a whole-world raw preview, blurred to each shot's view; " +
  "each shot looks where that activity was greatest over the shot, discounting regions already shown";

/** The rule `scripts/media.ts --camera fixed` records: no director, one key held. */
export const FIXED_RULE = "fixed: the torus centre held at one zoom for the whole film";

const GRID = 128;

/** A PNG's luminance as `GRID × GRID` cell means (the frame must be square, a multiple of GRID). */
export function luminance(path: string): Float64Array {
  const decoded = spawnSync("ffmpeg", ["-loglevel", "error", "-i", path, "-f", "rawvideo", "-pix_fmt", "gray", "-"], {
    maxBuffer: 1 << 28,
  });
  if (decoded.status !== 0) throw new Error(`ffmpeg could not decode ${path}: ${decoded.stderr}`);
  const pixels = decoded.stdout as Buffer;
  const side = Math.round(Math.sqrt(pixels.length));
  if (side * side !== pixels.length || side % GRID !== 0) {
    throw new Error(`${path} is ${pixels.length} grey pixels, not a square multiple of ${GRID}`);
  }
  const block = side / GRID;
  const out = new Float64Array(GRID * GRID);
  for (let y = 0; y < side; y++) {
    const row = Math.floor(y / block) * GRID;
    for (let x = 0; x < side; x++) {
      const cell = row + Math.floor(x / block);
      out[cell] = (out[cell] ?? 0) + (pixels[y * side + x] ?? 0);
    }
  }
  for (let i = 0; i < out.length; i++) out[i] = (out[i] ?? 0) / (block * block);
  return out;
}

/** `field` box-blurred with radius `r` cells on the torus (separable, two passes). */
function blur(field: Float64Array, r: number): Float64Array {
  const pass = (input: Float64Array, horizontal: boolean) => {
    const out = new Float64Array(input.length);
    const width = 2 * r + 1;
    for (let line = 0; line < GRID; line++) {
      const at = (i: number) => {
        const k = ((i % GRID) + GRID) % GRID;
        return horizontal ? line * GRID + k : k * GRID + line;
      };
      let sum = 0;
      for (let i = -r; i <= r; i++) sum += input[at(i)] ?? 0;
      for (let i = 0; i < GRID; i++) {
        out[at(i)] = sum / width;
        sum += (input[at(i + r + 1)] ?? 0) - (input[at(i - r)] ?? 0);
      }
    }
    return out;
  };
  return pass(pass(field, true), false);
}

/** Each frame's activity map; frames at the ends copy their neighbour's. */
export function activities(frames: Float64Array[]): Float64Array[] {
  const out: Float64Array[] = [];
  for (let t = 0; t < frames.length; t++) {
    const before = frames[Math.max(0, t - 1)] as Float64Array;
    const after = frames[Math.min(frames.length - 1, t + 1)] as Float64Array;
    const change = new Float64Array(GRID * GRID);
    for (let i = 0; i < change.length; i++) change[i] = (after[i] ?? 0) - (before[i] ?? 0);
    const smoothed = blur(change, 2);
    for (let i = 0; i < smoothed.length; i++) smoothed[i] = Math.abs(smoothed[i] ?? 0);
    out.push(smoothed);
  }
  return out;
}

/**
 * `field` smoothed to about radius `r` with a single central peak: three box
 * passes of a third of the radius approximate a Gaussian, where one wide box
 * would leave a plateau whose first cell, not its center, is the maximum.
 */
function smooth(field: Float64Array, r: number): Float64Array {
  const third = Math.max(1, Math.round(r / 3));
  return blur(blur(blur(field, third), third), third);
}

/** The cell with the largest value, as a world point at its cell center. */
function peak(map: Float64Array, extent: [number, number]): [number, number] {
  let best = 0;
  for (let i = 1; i < map.length; i++) if ((map[i] ?? 0) > (map[best] ?? 0)) best = i;
  const [x, y] = [best % GRID, Math.floor(best / GRID)];
  return [((x + 0.5) * extent[0]) / GRID, ((y + 0.5) * extent[1]) / GRID];
}

/** Activity summed over frames [from, to). */
function summed(maps: Float64Array[], from: number, to: number): Float64Array {
  const out = new Float64Array(GRID * GRID);
  for (let t = Math.max(0, from); t < Math.min(maps.length, to); t++) {
    const map = maps[t] as Float64Array;
    for (let i = 0; i < out.length; i++) out[i] = (out[i] ?? 0) + (map[i] ?? 0);
  }
  return out;
}

/** Torus distance in cells between two world points. */
function cells(a: [number, number], b: [number, number], extent: [number, number]): number {
  let total = 0;
  for (const axis of [0, 1] as const) {
    const length = extent[axis];
    let d = Math.abs(a[axis] - b[axis]) % length;
    d = Math.min(d, length - d);
    total += ((d * GRID) / length) ** 2;
  }
  return Math.sqrt(total);
}

/**
 * The camera path for frames at `ticks` of a torus `extent` world units a
 * side, from the preview's activity `maps` (one per frame).
 */
export function direct(
  ticks: number[],
  maps: Float64Array[],
  extent: [number, number],
  options: DirectorOptions,
): CameraKey[] {
  if (ticks.length !== maps.length) throw new Error(`${ticks.length} ticks for ${maps.length} activity maps`);
  const tick = (frame: number) => ticks[Math.min(ticks.length - 1, Math.max(0, frame))] as number;
  const whole: [number, number] = [extent[0] / 2, extent[1] / 2];
  const keys: CameraKey[] = [{ tick: tick(0), center: whole, zoom: 1 }];
  const push = (frame: number, center: [number, number], zoom: number) => {
    const at = tick(frame);
    const last = keys[keys.length - 1] as CameraKey;
    if (at > last.tick) keys.push({ tick: at, center, zoom });
  };
  push(options.opening, whole, 1);
  const shown: [number, number][] = [];
  let frame = options.opening;
  let shot = 0;
  const end = ticks.length - 1 - options.closing - options.transition;
  while (frame + options.transition + options.dwell <= end) {
    const zoom = options.zooms[shot % options.zooms.length] as number;
    // Blur the shot's activity to about half the view's half-width, so the
    // peak is a region the view holds with margin, not a single busy cell.
    const radius = Math.max(1, Math.round(GRID / (2 * zoom) / 2));
    const begin = frame + options.transition;
    const map = smooth(summed(maps, begin, begin + options.dwell), radius);
    // Discount what earlier shots showed, within a view's width of it: the
    // last shot's region by 0.8, each one before it by half as much again.
    for (let i = 0; i < map.length; i++) {
      const at: [number, number] = [((i % GRID) + 0.5) * (extent[0] / GRID), (Math.floor(i / GRID) + 0.5) * (extent[1] / GRID)];
      shown.forEach((seen, index) => {
        const recency = shown.length - 1 - index;
        const d = cells(at, seen, extent);
        const near = Math.exp(-(d * d) / (2 * (GRID / (2 * zoom)) ** 2));
        map[i] = (map[i] ?? 0) * (1 - 0.8 * 0.5 ** recency * near);
      });
    }
    const target = peak(map, extent);
    // Drift towards the activity of the shot's second half, at most a
    // quarter of the view on each axis. The drifted center may leave the
    // torus; `flife render --camera` wraps it.
    const late = smooth(summed(maps, begin + Math.floor(options.dwell / 2), begin + options.dwell), radius);
    const towards = peak(late, extent);
    const drift: [number, number] = [0, 0];
    for (const axis of [0, 1] as const) {
      const length = extent[axis];
      const limit = length / zoom / 4;
      let d = (towards[axis] - target[axis]) % length;
      if (d > length / 2) d -= length;
      if (d < -length / 2) d += length;
      drift[axis] = Math.max(-limit, Math.min(limit, d));
    }
    push(begin, target, zoom);
    push(begin + options.dwell, [target[0] + drift[0], target[1] + drift[1]], zoom);
    shown.push(target);
    frame = begin + options.dwell;
    shot += 1;
  }
  push(frame + options.transition, whole, 1);
  push(ticks.length - 1, whole, 1);
  return keys;
}

/**
 * The frame the social card is cut from: the middle of the film's closest
 * shot (the first shot at the path's highest zoom), where single bodies
 * show. `ticks` are the film's frames' ticks, in order. A path that never
 * leaves the whole torus gives the first frame.
 */
export function cardFrame(keys: readonly CameraKey[], ticks: readonly number[]): number {
  const zoom = Math.max(1, ...keys.map((key) => key.zoom));
  const first = zoom > 1 ? keys.findIndex((key) => key.zoom === zoom) : -1;
  if (first < 0) return 0;
  let last = first;
  while (keys[last + 1]?.zoom === zoom) last += 1;
  const middle = ((keys[first]?.tick ?? 0) + (keys[last]?.tick ?? 0)) / 2;
  let best = 0;
  ticks.forEach((tick, frame) => {
    if (Math.abs(tick - middle) < Math.abs((ticks[best] ?? 0) - middle)) best = frame;
  });
  return best;
}
