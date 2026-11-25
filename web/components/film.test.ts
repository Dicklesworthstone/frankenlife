// The film's readout names the frame on screen: a frame's presentation time, which reaches the page rounded to whole
// microseconds either way, and any time inside the frame, must give that frame.

import { describe, expect, test } from "bun:test";

import { film, films } from "@/lib/media";

import { cameraSpans } from "./camera-track";
import { frameAt, scaleBarWidthCqw, zoomView } from "./film";

const { frames, fps } = film.video;

describe("frameAt", () => {
  test("every frame from its own time, rounded down or up to a microsecond", () => {
    for (let frame = 0; frame < frames; frame++) {
      const micros = (frame / fps) * 1e6;
      expect(frameAt(Math.floor(micros) / 1e6)).toBe(frame);
      expect(frameAt(Math.ceil(micros) / 1e6)).toBe(frame);
    }
  });

  test("every frame from its middle, where a seek lands", () => {
    for (let frame = 0; frame < frames; frame++) expect(frameAt((frame + 0.5) / fps)).toBe(frame);
  });

  test("a time just before a frame still shows the one before it", () => {
    for (let frame = 1; frame < frames; frame++) expect(frameAt(frame / fps - 0.001)).toBe(frame - 1);
  });

  test("times outside the film show its first or last frame", () => {
    expect(frameAt(-1)).toBe(0);
    expect(frameAt(frames / fps + 5)).toBe(frames - 1);
  });

  test("60 fps microsecond presentation times map to every frame of a 1,601-frame film (26.7 s)", () => {
    const fps60 = 60;
    const frames60 = 1601;
    for (let frame = 0; frame < frames60; frame++) {
      const micros = (frame / fps60) * 1e6;
      expect(frameAt(Math.floor(micros) / 1e6, fps60, frames60)).toBe(frame);
      expect(frameAt(Math.ceil(micros) / 1e6, fps60, frames60)).toBe(frame);
      expect(frameAt((frame + 0.5) / fps60, fps60, frames60)).toBe(frame);
    }
    for (let frame = 1; frame < frames60; frame++) {
      expect(frameAt(frame / fps60 - 0.0005, fps60, frames60)).toBe(frame - 1);
    }
    expect(frameAt(-0.1, fps60, frames60)).toBe(0);
    expect(frameAt(30, fps60, frames60)).toBe(frames60 - 1);
  });
});

describe("scaleBarWidthCqw", () => {
  test("matches 1:1 square frames at 1080p and 2160p identically", () => {
    expect(scaleBarWidthCqw(10, 1, [80, 80], [1080, 1080])).toBeCloseTo(12.5, 10);
    expect(scaleBarWidthCqw(10, 1, [80, 80], [2160, 2160])).toBeCloseTo(12.5, 10);
    expect(scaleBarWidthCqw(5, 4, [80, 80], [2160, 2160])).toBeCloseTo(25, 10);
  });

  test("scales by H/W (9/16) on 16:9 4K frames of a square torus to match Viewport::fit", () => {
    expect(scaleBarWidthCqw(10, 1, [80, 80], [3840, 2160])).toBeCloseTo(12.5 * (9 / 16), 10);
    expect(scaleBarWidthCqw(5, 4, [80, 80], [3840, 2160])).toBeCloseTo(25 * (9 / 16), 10);
  });
});

describe("zoomView", () => {
  test("names the whole torus only at zoom 1, and a held 1.03 as itself", () => {
    expect(zoomView(1)).toEqual({ short: "whole", long: "the whole torus" });
    expect(zoomView(1.03)).toEqual({ short: "×1.03", long: "zoom 1.03" });
    expect(zoomView(1.6)).toEqual({ short: "×1.6", long: "zoom 1.6" });
    expect(zoomView(2.97)).toEqual({ short: "×3.0", long: "zoom 3.0" });
  });
});

describe("cameraSpans", () => {
  // The 1,601-frame films: a frame every 2 ticks at 60 fps, 35% whole-torus opening, two shots, and the rest (at
  // least 10%; 18% here, as no third shot fits) whole at the close.
  const keys = [
    { tick: 2400, zoom: 1 },
    { tick: 3520, zoom: 1 },
    { tick: 3680, zoom: 1.6 },
    { tick: 4192, zoom: 1.6 },
    { tick: 4352, zoom: 3 },
    { tick: 4864, zoom: 3 },
    { tick: 5024, zoom: 1 },
    { tick: 5600, zoom: 1 },
  ];

  test("holds and moves tile the film exactly, in its own seconds", () => {
    const spans = cameraSpans(keys, 2400, 5600, 2, 60);
    expect(spans.map((span) => span.kind)).toEqual(["hold", "move", "hold", "move", "hold", "move", "hold"]);
    expect(spans.map((span) => span.zoom)).toEqual([1, 1.6, 1.6, 3, 3, 1, 1]);
    expect(spans.reduce((sum, span) => sum + span.share, 0)).toBeCloseTo(1, 12);
    expect(spans[0]?.start).toBe(0);
    for (let at = 1; at < spans.length; at++) {
      expect(spans[at]?.start).toBeCloseTo((spans[at - 1]?.start ?? 0) + (spans[at - 1]?.share ?? 0), 12);
    }
    // 1,120 ticks of whole torus at a frame every 2 ticks and 60 fps: 9.33 s.
    expect(spans[0]?.seconds).toBeCloseTo(1120 / 2 / 60, 12);
    expect(spans.reduce((sum, span) => sum + span.seconds, 0)).toBeCloseTo(3200 / 2 / 60, 12);
  });

  test("every film on the site opens on the whole torus for at least a third of its length before it moves in", () => {
    for (const each of films) {
      const { world, video, render } = each;
      const spans = cameraSpans(render.camera?.keys ?? [], world.from.tick, world.to.tick, world.every, video.fps);
      expect(spans[0]?.kind).toBe("hold");
      expect(spans[0]?.zoom).toBe(1);
      expect(spans[0]?.share ?? 0).toBeGreaterThanOrEqual(1 / 3);
      expect(spans[0]?.seconds ?? 0).toBeGreaterThanOrEqual(9);
      // And it ends there too.
      expect(spans.at(-1)?.kind).toBe("hold");
      expect(spans.at(-1)?.zoom).toBe(1);
    }
  });

  test("a film with no path, or a bad range, has no spans; repeated keys add none", () => {
    expect(cameraSpans([], 0, 100, 1, 60)).toEqual([]);
    expect(cameraSpans([{ tick: 0, zoom: 1 }], 0, 100, 1, 60)).toEqual([]);
    expect(cameraSpans(keys, 5600, 2400, 2, 60)).toEqual([]);
    expect(cameraSpans([{ tick: 0, zoom: 1 }, { tick: 0, zoom: 2 }, { tick: 10, zoom: 2 }], 0, 10, 1, 10)).toHaveLength(1);
  });
});
