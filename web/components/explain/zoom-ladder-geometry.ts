// The geometry of "From the world to one brain" (zoom-ladder.tsx): where the
// film's poster and social card sit on the torus, the organism the zoom lands
// on, and the camera between the stops. Pure, so `bun test` checks it.
//
// Sources:
// - The torus is 80 × 80 world units with a 320 × 320 field, and a body is
//   0.25 units across: presets/confluent-world.toml, [world] and [contact].
// - The poster is film frame 0 and the card is cut from frame `social.frame`
//   (lib/films/confluent.json). A frame is 2160 × 2160 and shows 80/zoom world units
//   across, y down as the world's (crates/flife-cli/src/camera.rs).
// - The card is that frame scaled to 1200 wide and centre-cropped to 1200 × 630
//   (scripts/media.ts: `scale=1200:-1,crop=1200:630`).

import type { FilmManifest } from "@/lib/media";

export const TORUS = 80;
export const FIELD_CELLS = 320;
export const BODY = 0.25;
export const FRAME_PX = 2160;
export const CARD_W = 1200;
export const CARD_H = 630;

type Key = { tick: number; center: [number, number]; zoom: number };

/**
 * The film camera at `tick`, as crates/flife-cli/src/camera.rs computes it:
 * hold before the first key and after the last; between two keys ease with
 * smoothstep, the zoom geometrically and the centre the shorter way round.
 */
export function cameraAt(keys: readonly Key[], tick: number): { center: [number, number]; zoom: number } {
  // camera.rs's `wrap`: onto [0, TORUS).
  const wrap = (value: number) => ((value % TORUS) + TORUS) % TORUS;
  const first = keys[0];
  const last = keys.at(-1);
  if (!first || !last) return { center: [TORUS / 2, TORUS / 2], zoom: 1 };
  if (tick <= first.tick) return { center: [wrap(first.center[0]), wrap(first.center[1])], zoom: first.zoom };
  if (tick >= last.tick) return { center: [wrap(last.center[0]), wrap(last.center[1])], zoom: last.zoom };
  let index = 0;
  while ((keys[index + 1]?.tick ?? Infinity) <= tick) index += 1;
  const from = keys[index] ?? first;
  const to = keys[index + 1] ?? last;
  const fraction = (tick - from.tick) / (to.tick - from.tick);
  const eased = fraction * fraction * (3 - 2 * fraction);
  const zoom = Math.exp(Math.log(from.zoom) + eased * (Math.log(to.zoom) - Math.log(from.zoom)));
  const center: [number, number] = [0, 0];
  for (const axis of [0, 1] as const) {
    // As camera.rs: the step wrapped onto [0, TORUS), then into (−TORUS/2, TORUS/2].
    let step = wrap(to.center[axis] - from.center[axis]);
    if (step > TORUS / 2) step -= TORUS;
    center[axis] = wrap(from.center[axis] + eased * step);
  }
  return { center, zoom };
}

/** A picture's place on the torus: its top-left corner and its width and height, in world units. */
export type Placement = { x0: number; y0: number; width: number; height: number; px: [number, number] };

/** The poster: frame 0 at the camera's opening view (the whole torus at zoom 1). */
export function posterPlacement(film: FilmManifest): Placement & { tick: number } {
  const tick = film.world.ticks[film.poster.frame] ?? film.world.from.tick;
  const view = cameraAt(film.render.camera?.keys ?? [], tick);
  const span = TORUS / view.zoom;
  return {
    x0: view.center[0] - span / 2,
    y0: view.center[1] - span / 2,
    width: span,
    height: span,
    px: [film.video.width, film.video.height],
    tick,
  };
}

/** The social card: frame `social.frame`, scaled to 1200 wide and centre-cropped to 1200 × 630. */
export function cardPlacement(film: FilmManifest): Placement & { tick: number; zoom: number; center: [number, number] } {
  const tick = film.world.ticks[film.social.frame] ?? film.world.from.tick;
  const view = cameraAt(film.render.camera?.keys ?? [], tick);
  const span = TORUS / view.zoom;
  const cardW = film.social.width || CARD_W;
  const cardH = film.social.height || CARD_H;
  const height = span * (cardH / cardW);
  return {
    x0: view.center[0] - span / 2,
    y0: view.center[1] - height / 2,
    width: span,
    height,
    px: [cardW, cardH],
    tick,
    zoom: view.zoom,
    center: view.center,
  };
}

/** A card pixel's position on the torus. */
export function cardPixelToWorld(card: Placement, x: number, y: number): [number, number] {
  return [card.x0 + (x / card.px[0]) * card.width, card.y0 + (y / card.px[1]) * card.height];
}

/**
 * The organism the zoom lands on, in the card's pixels: a bright lime sphere
 * with darker space and other lineages around it, near the card's centre.
 * Chosen offline by scanning the card for a bright disc (radius 3 px) inside a
 * dark ring (radius 8–10 px), ranking by the contrast, and picking by eye
 * among the strongest. It belongs to the card with this SHA-256; a new film
 * needs a new pick (zoom-ladder-geometry.test.ts says so).
 */
export const PICK = { x: 667, y: 445, cardSha256: "8f0b33d8b0b2b68c17ccdbe0a9dc10ff61d026e67cd6f325aa12d8f11b0edbf4" } as const;

/** A camera over the torus: the centre of the view and its width in world units (the view is square). */
export type View = { cx: number; cy: number; span: number };

/** The stops' views: the whole torus, the card's middle square, a few bodies around the pick, inside the pick. */
export function stopViews(film: FilmManifest): View[] {
  const poster = posterPlacement(film);
  const card = cardPlacement(film);
  const [ox, oy] = cardPixelToWorld(card, PICK.x, PICK.y);
  return [
    { cx: poster.x0 + poster.width / 2, cy: poster.y0 + poster.height / 2, span: poster.width },
    { cx: card.center[0], cy: card.center[1], span: card.height },
    { cx: ox, cy: oy, span: 3 },
    { cx: ox, cy: oy, span: 0.45 },
  ];
}

export const smoothstep = (t: number) => {
  const x = Math.min(1, Math.max(0, t));
  return x * x * (3 - 2 * x);
};

/** `t` mapped from [a, b] to [0, 1], clamped. */
export const ramp = (t: number, a: number, b: number) => Math.min(1, Math.max(0, (t - a) / (b - a)));

/**
 * The view at progress `p` (0 the whole torus … 3 inside the organism; it holds
 * after 3). The span changes geometrically; the centre moves as the span
 * closes, so the destination slides to the middle while the zoom brings it near.
 */
export function viewAt(views: readonly View[], p: number): View {
  const clamped = Math.min(views.length - 1, Math.max(0, p));
  const index = Math.min(views.length - 2, Math.floor(clamped));
  const from = views[index];
  const to = views[index + 1];
  if (!from || !to) return views[0] ?? { cx: TORUS / 2, cy: TORUS / 2, span: TORUS };
  const eased = smoothstep(clamped - index);
  const span = Math.exp(Math.log(from.span) + eased * (Math.log(to.span) - Math.log(from.span)));
  const along = from.span === to.span ? eased : (from.span - span) / (from.span - to.span);
  return { cx: from.cx + along * (to.cx - from.cx), cy: from.cy + along * (to.cy - from.cy), span };
}

/**
 * Scroll progress `u` in [0, 1] to stop progress in [0, stops − 1], holding
 * still around each stop so a reader can read it before the next zoom begins.
 */
export function plateau(u: number, stops: number, hold = 0.22): number {
  const t = Math.min(1, Math.max(0, u)) * (stops - 1);
  const index = Math.min(stops - 2, Math.floor(t));
  const fraction = t - index;
  return index + smoothstep((fraction - hold) / (1 - 2 * hold));
}

/** The longest round scale bar no wider than `fraction` of the view: 40, 20, 10, 5, 2, 1, 0.5 or 0.25 units. */
export function scaleBar(span: number, fraction = 0.3): number {
  const lengths = [40, 20, 10, 5, 2, 1, 0.5, 0.25];
  return lengths.find((length) => length <= span * fraction) ?? 0.25;
}

/** `n` to two significant figures: 8,028 → 8,000; 369 → 370. */
export function roughly(n: number): number {
  if (n <= 0) return 0;
  const magnitude = 10 ** (Math.floor(Math.log10(n)) - 1);
  return Math.round(n / magnitude) * magnitude;
}

/** Organisms in a square view of `span` units at the frame's average density: live ÷ the torus's area × the view's. */
export function inView(live: number, span: number): number {
  return (live / (TORUS * TORUS)) * span * span;
}

/** Each layer's parameters, weights then biases, inputs to outputs: [6,720, 2,080, 1,188] for 104 → 64 → 32 → 36. */
export function layerSizes(widths: readonly number[]): number[] {
  const sizes: number[] = [];
  for (let index = 1; index < widths.length; index++) {
    const fanIn = widths[index - 1] ?? 0;
    const fanOut = widths[index] ?? 0;
    sizes.push(fanIn * fanOut + fanOut);
  }
  return sizes;
}
