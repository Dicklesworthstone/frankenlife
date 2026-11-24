import { describe, expect, test } from "bun:test";

import { parameterCount, widths, CATALOG, DEFAULT_TOPOLOGY } from "@/components/explain/brain-math";
import {
  cameraAt,
  cardPixelToWorld,
  cardPlacement,
  inView,
  layerSizes,
  PICK,
  plateau,
  posterPlacement,
  roughly,
  scaleBar,
  stopViews,
  viewAt,
} from "@/components/explain/zoom-ladder-geometry";
import { film } from "@/lib/media";

describe("the film's pictures on the torus", () => {
  test("the poster is frame 0 at tick 2,400: the whole 80-unit torus", () => {
    const poster = posterPlacement(film);
    expect(poster.tick).toBe(2400);
    expect([poster.x0, poster.y0, poster.width, poster.height]).toEqual([0, 0, 80, 80]);
  });

  test("the camera matches flife render's own record of each frame's zoom", () => {
    const keys = film.render.camera?.keys ?? [];
    film.world.ticks.forEach((tick, frame) => {
      const recorded = film.render.zoom?.[frame];
      if (recorded !== undefined) expect(cameraAt(keys, tick).zoom).toBeCloseTo(recorded, 2);
    });
  });

  test("the camera crosses a seam as camera.rs does, half a torus apart included", () => {
    // camera.rs wraps the step onto [0, 80) and then into (−40, 40]: 50 → 10 goes 40 east, through 70.
    const half = cameraAt(
      [
        { tick: 0, center: [50, 40], zoom: 1 },
        { tick: 100, center: [10, 40], zoom: 1 },
      ],
      50,
    );
    expect(half.center[0]).toBeCloseTo(70, 12);
    // 78 → 2 is 4 east, across the seam; a key outside the torus holds wrapped.
    const seam = cameraAt(
      [
        { tick: 0, center: [78, 40], zoom: 1 },
        { tick: 100, center: [82, 40], zoom: 1 },
      ],
      50,
    );
    expect(seam.center[0]).toBeCloseTo(0, 12);
    expect(cameraAt([{ tick: 0, center: [82, -4], zoom: 2 }], 10).center).toEqual([2, 76]);
  });

  test("the card is frame 368 at tick 3,872, zoom 3, halfway between two keys: centre (27.8125, 67.5)", () => {
    const card = cardPlacement(film);
    expect(film.social.frame).toBe(368);
    expect(card.tick).toBe(3872);
    expect(card.zoom).toBeCloseTo(3, 12);
    expect(card.center[0]).toBeCloseTo(27.8125, 12);
    expect(card.center[1]).toBeCloseTo(67.5, 12);
    // 80/3 units wide; 630/1200 of that tall: 14 units.
    expect(card.width).toBeCloseTo(80 / 3, 12);
    expect(card.height).toBeCloseTo(14, 12);
  });

  test("the picked organism belongs to this card, and sits at (29.30, 70.39)", () => {
    // A new film has a new card: pick a new organism (zoom-ladder-geometry.ts, PICK).
    expect(film.social.sha256).toBe(PICK.cardSha256);
    const [x, y] = cardPixelToWorld(cardPlacement(film), PICK.x, PICK.y);
    expect(x).toBeCloseTo(29.3, 1);
    expect(y).toBeCloseTo(70.39, 1);
  });
});

describe("the camera between the stops", () => {
  const views = stopViews(film);

  test("each stop's view is exact at its stop", () => {
    views.forEach((view, index) => {
      const at = viewAt(views, index);
      expect(at.span).toBeCloseTo(view.span, 9);
      expect(at.cx).toBeCloseTo(view.cx, 9);
      expect(at.cy).toBeCloseTo(view.cy, 9);
    });
  });

  test("the span shrinks monotonically from 80 units to inside one body", () => {
    let last = Infinity;
    for (let p = 0; p <= 3; p += 0.01) {
      const { span } = viewAt(views, p);
      expect(span).toBeLessThanOrEqual(last + 1e-9);
      last = span;
    }
    expect(viewAt(views, 3).span).toBeLessThan(0.5);
  });

  test("scrolling holds at each stop and passes through every stop in order", () => {
    expect(plateau(0, 5)).toBe(0);
    expect(plateau(1, 5)).toBe(4);
    expect(plateau(0.25, 5)).toBeCloseTo(1, 12);
    expect(plateau(0.26, 5)).toBeCloseTo(1, 12);
    let last = -1;
    for (let u = 0; u <= 1; u += 0.001) {
      const p = plateau(u, 5);
      expect(p).toBeGreaterThanOrEqual(last - 1e-12);
      last = p;
    }
  });
});

describe("the figures the stops quote", () => {
  test("organisms in view at the frame's average density", () => {
    expect(inView(262_144, 80)).toBe(262_144);
    expect(roughly(inView(262_144, 14))).toBe(8000);
    expect(roughly(inView(262_144, 3))).toBe(370);
  });

  test("scale bars", () => {
    expect(scaleBar(80)).toBe(20);
    expect(scaleBar(14)).toBe(2);
    expect(scaleBar(3)).toBe(0.5);
  });

  test("the founders' network: 9,988 parameters in three layers", () => {
    const shape = widths(CATALOG[DEFAULT_TOPOLOGY] ?? { hidden: [] });
    expect(shape).toEqual([104, 64, 32, 36]);
    expect(layerSizes(shape)).toEqual([6720, 2080, 1188]);
    expect(parameterCount(shape)).toBe(9988);
  });
});
