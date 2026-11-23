// The film's readout names the frame on screen: a frame's presentation time, which reaches the page rounded to whole
// microseconds either way, and any time inside the frame, must give that frame.

import { describe, expect, test } from "bun:test";

import { film } from "@/lib/media";

import { frameAt, scaleBarWidthCqw } from "./film";

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

  test("60 fps microsecond presentation times map cleanly to exact frames", () => {
    const fps60 = 60;
    const frames60 = 600;
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
    expect(frameAt(15, fps60, frames60)).toBe(frames60 - 1);
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
