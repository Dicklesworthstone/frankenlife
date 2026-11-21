// The state machine behind <TickPipeline>: one candidate tick prepared
// against a committed world and published whole, or not at all (plan
// §8.1–§8.3, fixture C.8; crates/flife-engine/src/world.rs). It is a diagram
// of the engine's rules, not the engine: no world is stepped here.

/** The ten ordered phases of plan §8.1. */
export const PHASES = 10;
/** Phase 8 resolves capacity: a birth the fixed quotas cannot cover pauses the tick there. */
export const PAUSE_PHASE = 8;
/** Where this diagram raises its numerical fault. Any phase of the engine can raise one. */
export const FAULT_PHASE = 6;

/**
 * - `clean`: the candidate passes every phase and its checks, and publishes.
 * - `capacity`: under `pause-before-commit`, a birth the quotas cannot cover pauses the tick.
 * - `fault`: under `stop-and-preserve`, a numerical fault stops the run.
 */
export type Scenario = "clean" | "capacity" | "fault";

/**
 * - `idle`: no candidate; the committed world is all there is.
 * - `preparing`: a candidate for tick + 1 is in phase `phase`.
 * - `published`, `cancelled`, `paused`: how the last candidate ended (at `at`).
 * - `stopped`: a numerical fault stopped the run; every later advance is refused.
 */
export type Status = "idle" | "preparing" | "published" | "cancelled" | "paused" | "stopped";

export type TickState = {
  /** The committed tick n. */
  tick: number;
  /** The committed epoch: one more per publication. */
  epoch: number;
  /** The phase the candidate is in, 1 to 10; 0 with no candidate. */
  phase: number;
  status: Status;
  scenario: Scenario;
  /** The phase at which the last candidate ended (published at 10, cancelled, paused or faulted earlier); 0 before any. */
  at: number;
};

export type TickAction =
  | { type: "start" }
  | { type: "advance" }
  | { type: "cancel" }
  | { type: "scenario"; scenario: Scenario }
  | { type: "reset"; tick: number };

export function initialTick(tick: number, scenario: Scenario = "clean"): TickState {
  return { tick, epoch: tick, phase: 0, status: "idle", scenario, at: 0 };
}

/** Whether a "Cancel" now discards the candidate: only before phase 10 publishes it. */
export function canCancel(state: TickState): boolean {
  return state.status === "preparing" && state.phase < PHASES;
}

/** Whether a new candidate can be prepared: never once the run has stopped. */
export function canStart(state: TickState): boolean {
  return state.status !== "preparing" && state.status !== "stopped";
}

export function tickReducer(state: TickState, action: TickAction): TickState {
  switch (action.type) {
    case "start":
      // A paused world retried unchanged prepares the same candidate again.
      return canStart(state) ? { ...state, phase: 1, status: "preparing" } : state;
    case "advance": {
      if (state.status !== "preparing") return state;
      const { phase, scenario } = state;
      if (scenario === "fault" && phase === FAULT_PHASE) {
        return { ...state, phase: 0, status: "stopped", at: phase };
      }
      if (scenario === "capacity" && phase === PAUSE_PHASE) {
        return { ...state, phase: 0, status: "paused", at: phase };
      }
      if (phase >= PHASES) {
        // Population, field, ledger, quota, identities, paths, tick and epoch, together.
        return { ...state, tick: state.tick + 1, epoch: state.epoch + 1, phase: 0, status: "published", at: PHASES };
      }
      return { ...state, phase: phase + 1 };
    }
    case "cancel":
      return canCancel(state) ? { ...state, phase: 0, status: "cancelled", at: state.phase } : state;
    case "scenario":
      // The scenario is the world's situation, chosen between candidates.
      return state.status === "preparing" ? state : { ...state, scenario: action.scenario };
    case "reset":
      return initialTick(action.tick, state.scenario);
  }
}
