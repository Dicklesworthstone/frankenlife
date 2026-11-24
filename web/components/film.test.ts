// The film's readout names the frame on screen: a frame's presentation time, which reaches the page rounded to whole
// microseconds either way, and any time inside the frame, must give that frame.

import { describe, expect, test } from "bun:test";

import { film } from "@/lib/media";

import { frameAt, scaleBarWidthCqw, zoomView } from "./film";
import {
  type DualSlotState,
  globalFrameFromSegmentTime,
  nextSegmentIndex,
  planSlotAdvance,
  planSlotSeek,
  resolveSegments,
  segmentLocationForGlobalFrame,
} from "./film-segments";

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

  test("60 fps microsecond presentation times map cleanly to all 801 exact frames (13.35s)", () => {
    const fps60 = 60;
    const frames60 = 801;
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

describe("multi-segment film timeline mapping", () => {
  const segs = resolveSegments(
    {
      src: "/media/film-seg00-aaaaaaaaaaaa.mp4",
      frames: 600,
      segments: [
        {
          src: "/media/film-seg00-aaaaaaaaaaaa.mp4",
          sha256: "a".repeat(64),
          bytes: 18_000_000,
          start_frame: 0,
          frames: 180,
          start_tick: 2400,
          end_tick: 3116,
        },
        {
          src: "/media/film-seg01-bbbbbbbbbbbb.mp4",
          sha256: "b".repeat(64),
          bytes: 20_000_000,
          start_frame: 180,
          frames: 240,
          start_tick: 3120,
          end_tick: 4076,
        },
        {
          src: "/media/film-seg02-cccccccccccc.mp4",
          sha256: "c".repeat(64),
          bytes: 17_500_000,
          start_frame: 420,
          frames: 180,
          start_tick: 4080,
          end_tick: 4796,
        },
      ],
    },
    { from: { tick: 2400 }, to: { tick: 4796 } },
  );

  test("resolves single-file manifests into one segment and multi-segment manifests into all segments", () => {
    const single = resolveSegments(
      { src: "/media/film-single.mp4", frames: 401 },
      { from: { tick: 2400 }, to: { tick: 5600 } },
    );
    expect(single).toEqual([
      { src: "/media/film-single.mp4", startFrame: 0, frames: 401, startTick: 2400, endTick: 5600 },
    ]);
    expect(segs).toHaveLength(3);
    expect(segs.map((s) => s.startFrame)).toEqual([0, 180, 420]);
  });

  test("maps every global frame <-> (segmentIndex, localTime) cleanly across 60 fps boundaries", () => {
    const fps60 = 60;
    for (let globalFrame = 0; globalFrame < 600; globalFrame += 1) {
      const loc = segmentLocationForGlobalFrame(segs, globalFrame, fps60);
      expect(globalFrameFromSegmentTime(segs, loc.segmentIndex, loc.localTime, fps60)).toBe(globalFrame);
      // Verify microsecond rounding at segment-local start timestamps too.
      const micros = (loc.localFrame / fps60) * 1e6;
      expect(globalFrameFromSegmentTime(segs, loc.segmentIndex, Math.floor(micros) / 1e6, fps60)).toBe(globalFrame);
      expect(globalFrameFromSegmentTime(segs, loc.segmentIndex, Math.ceil(micros) / 1e6, fps60)).toBe(globalFrame);
    }
    expect(segmentLocationForGlobalFrame(segs, -5, fps60).segmentIndex).toBe(0);
    expect(segmentLocationForGlobalFrame(segs, 9999, fps60)).toEqual({
      segmentIndex: 2,
      localFrame: 179,
      localTime: 179.5 / fps60,
    });
  });

  test("wraps nextSegmentIndex cleanly across loop boundaries", () => {
    expect(nextSegmentIndex(0, 3)).toBe(1);
    expect(nextSegmentIndex(1, 3)).toBe(2);
    expect(nextSegmentIndex(2, 3)).toBe(0);
    expect(nextSegmentIndex(0, 1)).toBe(0);
  });

  test("dual-slot advance loops 0 -> 1 -> 2 -> 0 -> 1 with zero-load active swaps and ignores stale slot ended", () => {
    let state: DualSlotState = { activeSlot: 0, slotSegments: [0, 1] };
    // Stale ended event from inactive slot 1 must be ignored.
    expect(planSlotAdvance(state, 1, 3)).toBeNull();

    // 0 -> 1 (slot 1 already buffered seg 1; retired slot 0 preloads seg 2).
    const t1 = planSlotAdvance(state, 0, 3)!;
    expect(t1.activeSegment).toBe(1);
    expect(t1.activeNeedsLoad).toBe(false);
    expect(t1.standbySegment).toBe(2);
    expect(t1.standbyNeedsLoad).toBe(true);
    expect(t1.shouldPlay).toBe(true);
    state = t1.nextState;
    expect(state).toEqual({ activeSlot: 1, slotSegments: [2, 1] });

    // 1 -> 2 (slot 0 already buffered seg 2; retired slot 1 preloads seg 0).
    const t2 = planSlotAdvance(state, 1, 3)!;
    expect(t2.activeSegment).toBe(2);
    expect(t2.activeNeedsLoad).toBe(false);
    expect(t2.standbySegment).toBe(0);
    expect(t2.standbyNeedsLoad).toBe(true);
    state = t2.nextState;
    expect(state).toEqual({ activeSlot: 0, slotSegments: [2, 0] });

    // 2 -> 0 (loop wrap! slot 1 already buffered seg 0; retired slot 0 preloads seg 1).
    const t3 = planSlotAdvance(state, 0, 3)!;
    expect(t3.activeSegment).toBe(0);
    expect(t3.activeNeedsLoad).toBe(false);
    expect(t3.standbySegment).toBe(1);
    expect(t3.standbyNeedsLoad).toBe(true);
    state = t3.nextState;
    expect(state).toEqual({ activeSlot: 1, slotSegments: [1, 0] });

    // 2-segment film loops forever with zero reloads on either slot!
    let twoState: DualSlotState = { activeSlot: 0, slotSegments: [0, 1] };
    for (let step = 0; step < 6; step += 1) {
      const tr = planSlotAdvance(twoState, twoState.activeSlot, 2)!;
      expect(tr.activeNeedsLoad).toBe(false);
      expect(tr.standbyNeedsLoad).toBe(false);
      twoState = tr.nextState;
    }
  });

  test("dual-slot seek handles same-segment, paused cross-segment, playing cross-segment, and multi-segment jumps", () => {
    const start: DualSlotState = { activeSlot: 0, slotSegments: [0, 1] };

    // Same-segment seek keeps slot and never reloads.
    const same = planSlotSeek(start, 0, false, 3);
    expect(same.changedSlot).toBe(false);
    expect(same.activeNeedsLoad).toBe(false);
    expect(same.shouldPlay).toBe(false);

    // Seek to preloaded segment 1 while PAUSED: instant swap, stays paused, preloads seg 2 in retired slot.
    const pausedNext = planSlotSeek(start, 1, false, 3);
    expect(pausedNext.changedSlot).toBe(true);
    expect(pausedNext.activeSegment).toBe(1);
    expect(pausedNext.activeNeedsLoad).toBe(false);
    expect(pausedNext.shouldPlay).toBe(false);
    expect(pausedNext.standbySegment).toBe(2);
    expect(pausedNext.nextState).toEqual({ activeSlot: 1, slotSegments: [2, 1] });

    // Seek across multiple segments (0 -> 2) while PLAYING: loads seg 2 into standby, resumes playing, preloads seg 0.
    const playingJump = planSlotSeek(start, 2, true, 3);
    expect(playingJump.changedSlot).toBe(true);
    expect(playingJump.activeSegment).toBe(2);
    expect(playingJump.activeNeedsLoad).toBe(true);
    expect(playingJump.shouldPlay).toBe(true);
    expect(playingJump.standbySegment).toBe(0);
    expect(playingJump.standbyNeedsLoad).toBe(false);
    expect(playingJump.nextState).toEqual({ activeSlot: 1, slotSegments: [0, 2] });
  });

  test("handles 801 true-60fps frames across 5 camera-cut segments (13.35s) and loops cleanly 0..4..0", () => {
    const segs5 = resolveSegments(
      {
        src: "/media/film-seg00-000000000000.mp4",
        frames: 801,
        segments: [
          { src: "/media/film-seg00-000000000000.mp4", sha256: "0".repeat(64), bytes: 18_000_000, start_frame: 0, frames: 120, start_tick: 2400, end_tick: 2876 },
          { src: "/media/film-seg01-111111111111.mp4", sha256: "1".repeat(64), bytes: 19_000_000, start_frame: 120, frames: 184, start_tick: 2880, end_tick: 3612 },
          { src: "/media/film-seg02-222222222222.mp4", sha256: "2".repeat(64), bytes: 20_000_000, start_frame: 304, frames: 184, start_tick: 3616, end_tick: 4348 },
          { src: "/media/film-seg03-333333333333.mp4", sha256: "3".repeat(64), bytes: 18_500_000, start_frame: 488, frames: 128, start_tick: 4352, end_tick: 4860 },
          { src: "/media/film-seg04-444444444444.mp4", sha256: "4".repeat(64), bytes: 19_500_000, start_frame: 616, frames: 185, start_tick: 4864, end_tick: 5600 },
        ],
      },
      { from: { tick: 2400 }, to: { tick: 5600 } },
    );
    const fps60 = 60;
    for (let f = 0; f < 801; f += 1) {
      const loc = segmentLocationForGlobalFrame(segs5, f, fps60);
      expect(globalFrameFromSegmentTime(segs5, loc.segmentIndex, loc.localTime, fps60)).toBe(f);
      const micros = (loc.localFrame / fps60) * 1e6;
      expect(globalFrameFromSegmentTime(segs5, loc.segmentIndex, Math.floor(micros) / 1e6, fps60)).toBe(f);
      expect(globalFrameFromSegmentTime(segs5, loc.segmentIndex, Math.ceil(micros) / 1e6, fps60)).toBe(f);
    }
    let state: DualSlotState = { activeSlot: 0, slotSegments: [0, 1] };
    const visited: number[] = [0];
    for (let step = 0; step < 6; step += 1) {
      const tr = planSlotAdvance(state, state.activeSlot, 5)!;
      expect(tr.activeNeedsLoad).toBe(false);
      visited.push(tr.activeSegment);
      state = tr.nextState;
    }
    expect(visited).toEqual([0, 1, 2, 3, 4, 0, 1]);
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
