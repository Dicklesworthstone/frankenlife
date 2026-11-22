// The film's readout names the frame on screen: a frame's presentation time, which reaches the page rounded to whole
// microseconds either way, and any time inside the frame, must give that frame.

import { describe, expect, test } from "bun:test";

import { film } from "@/lib/media";

import { frameAt } from "./film";

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
});
