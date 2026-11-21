import { describe, expect, test } from "bun:test";

import {
  FAULT_PHASE,
  PAUSE_PHASE,
  PHASES,
  type TickAction,
  type TickState,
  canCancel,
  initialTick,
  tickReducer,
} from "./tick-machine";

const run = (state: TickState, ...actions: TickAction[]) => actions.reduce(tickReducer, state);
const advance = (times: number): TickAction[] => Array.from({ length: times }, () => ({ type: "advance" }) as const);

describe("tick machine", () => {
  test("a clean candidate publishes tick and epoch together after phase 10", () => {
    const started = run(initialTick(1024), { type: "start" });
    const before = run(started, ...advance(PHASES - 1));
    expect(before).toMatchObject({ tick: 1024, epoch: 1024, phase: PHASES, status: "preparing" });
    const after = tickReducer(before, { type: "advance" });
    expect(after).toMatchObject({ tick: 1025, epoch: 1025, phase: 0, status: "published", at: PHASES });
  });

  test("cancelling before phase 10 discards the candidate and leaves the committed world", () => {
    for (let phase = 1; phase < PHASES; phase += 1) {
      const preparing = run(initialTick(7), { type: "start" }, ...advance(phase - 1));
      expect(canCancel(preparing)).toBe(true);
      const cancelled = tickReducer(preparing, { type: "cancel" });
      expect(cancelled).toMatchObject({ tick: 7, epoch: 7, phase: 0, status: "cancelled", at: phase });
    }
  });

  test("phase 10 cannot be cancelled", () => {
    const publishing = run(initialTick(7), { type: "start" }, ...advance(PHASES - 1));
    expect(canCancel(publishing)).toBe(false);
    expect(tickReducer(publishing, { type: "cancel" })).toBe(publishing);
  });

  test("a capacity shortfall pauses at phase 8, and the unchanged world pauses there again", () => {
    const world = initialTick(100, "capacity");
    const paused = run(world, { type: "start" }, ...advance(PAUSE_PHASE));
    expect(paused).toMatchObject({ tick: 100, epoch: 100, status: "paused", at: PAUSE_PHASE });
    const again = run(paused, { type: "start" }, ...advance(PAUSE_PHASE));
    expect(again).toMatchObject({ tick: 100, status: "paused", at: PAUSE_PHASE });
  });

  test("a numerical fault stops the run, keeps the committed tick and refuses every later advance", () => {
    const stopped = run(initialTick(100, "fault"), { type: "start" }, ...advance(FAULT_PHASE));
    expect(stopped).toMatchObject({ tick: 100, epoch: 100, status: "stopped", at: FAULT_PHASE });
    expect(run(stopped, { type: "start" })).toBe(stopped);
    expect(run(stopped, { type: "scenario", scenario: "clean" }, { type: "start" }).status).toBe("stopped");
    expect(run(stopped, { type: "reset", tick: 100 })).toMatchObject({ status: "idle", tick: 100 });
  });

  test("the scenario never changes under a candidate", () => {
    const preparing = run(initialTick(1), { type: "start" });
    expect(tickReducer(preparing, { type: "scenario", scenario: "fault" }).scenario).toBe("clean");
  });
});
