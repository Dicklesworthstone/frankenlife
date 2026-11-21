// The model behind <BranchDiagram>: which world each lane of a kept run
// holds at a tick, and a toy digest of it. It restates what flife's run
// directories promise (README FL-037, FL-039; crates/flife-cli/tests/resume.rs
// and tests/branch.rs): the same configuration, checkpoint and commands reach
// the same world. The digests are illustrative, a 32-bit FNV-1a of a label,
// never a world's real SHA-256 `frankenlife.world-state.v1` digest.

/** The diagram's run length and checkpoint interval, in ticks (illustrative; a preset sets its own interval). */
export const END = 4096;
export const INTERVAL = 512;

export type Lanes = {
  /** `flife branch run DIR --at T`: a checkpoint tick. */
  branchAt: number;
  /** `--inject B:X,Y:AMOUNT`: applied at tick B's boundary, so the world differs from tick B + 1. */
  brushAt: number;
  /** The tick at which a second run of the same configuration is SIGKILLed. */
  killAt: number;
};

export type LaneId = "run" | "resumed" | "control" | "brushed";

/** The newest checkpoint at or before `tick`. */
export function newestCheckpoint(tick: number, interval = INTERVAL): number {
  return Math.floor(tick / interval) * interval;
}

/**
 * The world `lane` holds at committed tick `tick`, as a label two lanes share
 * exactly when their worlds are the same; null where the lane has no world.
 */
export function worldAt(lane: LaneId, tick: number, lanes: Lanes): string | null {
  switch (lane) {
    case "run":
    case "resumed":
      // Killed and resumed from its newest checkpoint that restores, it reaches the uninterrupted world.
      return "run";
    case "control":
      return tick < lanes.branchAt ? null : "run";
    case "brushed":
      if (tick < lanes.branchAt) return null;
      return tick <= lanes.brushAt ? "run" : `run+inject@${lanes.brushAt}`;
  }
}

/** A toy digest: 32-bit FNV-1a of the world's label and tick, as eight hex digits. */
export function toyDigest(world: string, tick: number): string {
  let hash = 0x811c9dc5;
  for (const char of `${world}#${tick}`) {
    hash ^= char.codePointAt(0) ?? 0;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash.toString(16).padStart(8, "0");
}

/** A hue for a digest, so equal digests show as equal colours. */
export function digestHue(digest: string): number {
  return Number.parseInt(digest.slice(0, 4), 16) % 360;
}
