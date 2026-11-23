// Pure multi-segment timeline mapping for <Film> (tested by components/film.test.ts).
//
// Maps seamlessly between a unified global frame index across the full film and
// `(segmentIndex, localFrame, localTime)` across 1 or more <= 25 MiB MP4 segments.

import type { VideoSegment } from "@/lib/media";

export type PlayableSegment = {
  src: string;
  startFrame: number;
  frames: number;
  startTick: number;
  endTick: number;
};

/** Resolve a film manifest into 1 or more ordered playable segments. */
export function resolveSegments(
  video: { src: string; frames: number; segments?: VideoSegment[] },
  world: { from: { tick: number }; to: { tick: number } },
): PlayableSegment[] {
  if (video.segments && video.segments.length > 0) {
    return video.segments.map((seg) => ({
      src: seg.src,
      startFrame: seg.start_frame,
      frames: seg.frames,
      startTick: seg.start_tick,
      endTick: seg.end_tick,
    }));
  }
  return [
    {
      src: video.src,
      startFrame: 0,
      frames: video.frames,
      startTick: world.from.tick,
      endTick: world.to.tick,
    },
  ];
}

/**
 * Map a segment's local presentation time (seconds) to the film's global frame index.
 * Allows a thousandth of a frame for microsecond-rounded presentation timestamps
 * (`0.033333` at 30 fps, `0.016667` at 60 fps).
 */
export function globalFrameFromSegmentTime(
  segments: readonly PlayableSegment[],
  segmentIndex: number,
  localTimeSeconds: number,
  fps: number,
): number {
  if (segments.length === 0) return 0;
  const clampedIndex = Math.min(segments.length - 1, Math.max(0, segmentIndex));
  const seg = segments[clampedIndex]!;
  const maxLocal = Math.max(0, seg.frames - 1);
  const localFrame = Math.min(maxLocal, Math.max(0, Math.floor(localTimeSeconds * fps + 1e-3)));
  return seg.startFrame + localFrame;
}

/**
 * Map a unified global frame index onto its owning segment, local frame offset,
 * and local seek timestamp (middle of the frame's interval so rounding never lands
 * on a neighbour).
 */
export function segmentLocationForGlobalFrame(
  segments: readonly PlayableSegment[],
  globalFrame: number,
  fps: number,
): { segmentIndex: number; localFrame: number; localTime: number } {
  if (segments.length === 0) return { segmentIndex: 0, localFrame: 0, localTime: 0 };
  const lastSeg = segments[segments.length - 1]!;
  const totalFrames = lastSeg.startFrame + lastSeg.frames;
  const clamped = Math.min(Math.max(0, totalFrames - 1), Math.max(0, Math.floor(globalFrame)));

  for (let i = 0; i < segments.length; i += 1) {
    const seg = segments[i]!;
    if (clamped >= seg.startFrame && clamped < seg.startFrame + seg.frames) {
      const localFrame = clamped - seg.startFrame;
      return { segmentIndex: i, localFrame, localTime: (localFrame + 0.5) / fps };
    }
  }
  const fallbackLocal = Math.max(0, lastSeg.frames - 1);
  return {
    segmentIndex: segments.length - 1,
    localFrame: fallbackLocal,
    localTime: (fallbackLocal + 0.5) / fps,
  };
}

/** Next segment index in the loop (wraps cleanly from N-1 back to 0). */
export function nextSegmentIndex(segmentIndex: number, segmentCount: number): number {
  if (segmentCount <= 1) return 0;
  return (((segmentIndex + 1) % segmentCount) + segmentCount) % segmentCount;
}

export type SlotIndex = 0 | 1;

export type DualSlotState = {
  activeSlot: SlotIndex;
  slotSegments: [number, number];
};

export type SlotTransition = {
  nextState: DualSlotState;
  changedSlot: boolean;
  activeSegment: number;
  activeNeedsLoad: boolean;
  standbySlot: SlotIndex;
  standbySegment: number;
  standbyNeedsLoad: boolean;
  shouldPlay: boolean;
};

/**
 * Pure transition planner when `endedSlot` finishes playing in multi-segment mode.
 * Returns `null` if `endedSlot` is stale (not currently active) or there is only 1 segment.
 */
export function planSlotAdvance(
  state: DualSlotState,
  endedSlot: SlotIndex,
  segmentCount: number,
): SlotTransition | null {
  if (segmentCount <= 1 || endedSlot !== state.activeSlot) return null;
  const currentSeg = state.slotSegments[endedSlot];
  const targetSeg = nextSegmentIndex(currentSeg, segmentCount);
  const nextSlot: SlotIndex = endedSlot === 0 ? 1 : 0;
  const standbySeg = nextSegmentIndex(targetSeg, segmentCount);

  const activeNeedsLoad = state.slotSegments[nextSlot] !== targetSeg;
  const standbyNeedsLoad = state.slotSegments[endedSlot] !== standbySeg;
  const nextSlots: [number, number] = [state.slotSegments[0], state.slotSegments[1]];
  nextSlots[nextSlot] = targetSeg;
  nextSlots[endedSlot] = standbySeg;

  return {
    nextState: { activeSlot: nextSlot, slotSegments: nextSlots },
    changedSlot: true,
    activeSegment: targetSeg,
    activeNeedsLoad,
    standbySlot: endedSlot,
    standbySegment: standbySeg,
    standbyNeedsLoad,
    shouldPlay: true,
  };
}

/**
 * Pure transition planner when the user scrubs to `targetSegment` (paused or playing).
 * Keeps the active slot when seeking within the same segment; otherwise flips to the
 * standby slot (reusing it with zero reload if already preloaded!) and preloads
 * `(targetSegment + 1) % N` into the retired slot.
 */
export function planSlotSeek(
  state: DualSlotState,
  targetSegment: number,
  wasPlaying: boolean,
  segmentCount: number,
): SlotTransition {
  const curSlot = state.activeSlot;
  const curSeg = state.slotSegments[curSlot];
  const otherSlot: SlotIndex = curSlot === 0 ? 1 : 0;

  if (segmentCount <= 1 || targetSegment === curSeg) {
    return {
      nextState: {
        activeSlot: curSlot,
        slotSegments: [state.slotSegments[0], state.slotSegments[1]],
      },
      changedSlot: false,
      activeSegment: curSeg,
      activeNeedsLoad: false,
      standbySlot: otherSlot,
      standbySegment: state.slotSegments[otherSlot],
      standbyNeedsLoad: false,
      shouldPlay: wasPlaying,
    };
  }

  const standbySeg = nextSegmentIndex(targetSegment, segmentCount);
  const activeNeedsLoad = state.slotSegments[otherSlot] !== targetSegment;
  const standbyNeedsLoad = state.slotSegments[curSlot] !== standbySeg;
  const nextSlots: [number, number] = [state.slotSegments[0], state.slotSegments[1]];
  nextSlots[otherSlot] = targetSegment;
  nextSlots[curSlot] = standbySeg;

  return {
    nextState: { activeSlot: otherSlot, slotSegments: nextSlots },
    changedSlot: true,
    activeSegment: targetSegment,
    activeNeedsLoad,
    standbySlot: curSlot,
    standbySegment: standbySeg,
    standbyNeedsLoad,
    shouldPlay: wasPlaying,
  };
}
