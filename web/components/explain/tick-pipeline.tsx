"use client";

import { Ban, Check, OctagonX, CirclePause, Play, RotateCcw, SkipForward } from "lucide-react";
import { useEffect, useReducer, useRef, useState } from "react";

import { DemoButton, DemoFrame, Segmented } from "@/components/explain/demo";
import {
  FAULT_PHASE,
  PAUSE_PHASE,
  PHASES,
  type Scenario,
  type TickState,
  canCancel,
  canStart,
  initialTick,
  tickReducer,
} from "@/components/explain/tick-machine";
import { tickPhases } from "@/lib/content";
import { useInView, useReducedMotion } from "@/lib/hooks";
import { cn, grouped } from "@/lib/utils";

const START_TICK = 1024;
const PHASE_MS = 750;

/**
 * What each phase writes (crates/flife-engine/src/world.rs, the module's own
 * account). A candidate reads only committed state and writes only the spare
 * field buffer, the world's reusable scratch and its own copies.
 */
const WRITES = [
  "seals the commands due at this boundary",
  "writes the spare field buffer: the tick's one observation field",
  "writes reusable scratch: the pyramid and spatial bins",
  "writes scratch and its own copies: observations, held outputs",
  "writes reusable scratch: requests and cell grants",
  "writes its own copies: each organism's budget",
  "writes its own copies: motion, emission and split proposals",
  "stages births, their genomes held by the candidate; collects deaths",
  "writes the spare field buffer: grants debited, deposits and recycling added",
  "checks INV-01–03, 06 and 07, then publishes everything at once",
] as const;

/** The committed world's parts, which change only at publication (world.rs: Candidate::publish). */
const COMMITTED = ["population", "field", "ledger", "genome quota", "identities", "source paths"] as const;

/** The three places a candidate may write, and the first phase that writes each. */
const SCRATCH = [
  { label: "spare field buffer", from: 2 },
  { label: "reusable scratch", from: 3 },
  { label: "its own copies", from: 4 },
] as const;

const SCENARIOS = [
  { value: "clean", label: "Clean tick", tone: "rule" },
  { value: "capacity", label: "Capacity shortfall" },
  { value: "fault", label: "Numerical fault", tone: "wrong" },
] as const satisfies readonly { value: Scenario; label: string; tone?: "rule" | "wrong" }[];

function message(state: TickState): string {
  const next = grouped(state.tick + 1);
  const now = grouped(state.tick);
  switch (state.status) {
    case "idle":
      return `The committed world is tick ${now}. Nothing is being prepared.`;
    case "preparing":
      return `Preparing tick ${next}, phase ${state.phase}: ${tickPhases[state.phase - 1] ?? ""}. The committed world is untouched.`;
    case "published":
      return `Tick ${now} published: population, field, ledger, quota, identities, source paths, tick and epoch, together.`;
    case "cancelled":
      return `Cancelled in phase ${state.at}: the candidate for tick ${next} is discarded. The committed world is still tick ${now}, exactly as before.`;
    case "paused":
      return `Capacity shortfall in phase ${PAUSE_PHASE}: a birth the fixed quotas cannot cover. Under pause-before-commit the tick pauses and nothing changes; retrying the unchanged world prepares the same candidate.`;
    case "stopped":
      return `Numerical fault in phase ${state.at}: under stop-and-preserve the run stops. Tick ${now} is kept bit for bit, and every later advance is refused.`;
  }
}

/**
 * One tick as the engine runs it: a candidate prepared against the committed
 * world, phase by phase, then published whole, or discarded, paused or
 * stopped with the committed world untouched (plan §8.1–§8.3, fixture C.8).
 */
export function TickPipeline() {
  const [state, dispatch] = useReducer(tickReducer, START_TICK, (tick) => initialTick(tick));
  const reduced = useReducedMotion();
  const [mode, setMode] = useState<"animate" | "step">("animate");
  const frame = useRef<HTMLDivElement>(null);
  const inView = useInView(frame);

  // A reader who asks for less motion steps by hand.
  useEffect(() => {
    if (reduced) setMode("step");
  }, [reduced]);

  const animating = mode === "animate" && state.status === "preparing" && inView;
  useEffect(() => {
    if (!animating) return;
    const handle = window.setTimeout(() => dispatch({ type: "advance" }), state.phase === PHASES ? PHASE_MS * 1.4 : PHASE_MS);
    return () => window.clearTimeout(handle);
  }, [animating, state.phase]);

  const preparing = state.status === "preparing";
  // Phases the candidate has finished; while preparing, those before the current one.
  const done = preparing ? state.phase - 1 : state.status === "published" ? PHASES : state.status === "idle" ? 0 : state.at - 1;
  const ended = state.status === "cancelled" || state.status === "paused" || state.status === "stopped";

  return (
    <DemoFrame
      eyebrow="Plan §8.1–§8.3 · fixture C.8"
      title="Published whole, or not at all."
      basis="diagram"
      source={
        <>
          crates/flife-engine/src/world.rs: World::advance prepares a Candidate against the committed world;
          Candidate::publish commits the population as one checked transaction, then the rest, which cannot fail.
          Dropping or cancelling a candidate changes nothing. The phase where this diagram raises its fault is its
          own choice; any phase can.
        </>
      }
    >
      <div ref={frame} className="space-y-5">
        <div className="flex flex-wrap items-center gap-3">
          <Segmented
            label="What happens in this tick"
            options={SCENARIOS}
            value={state.scenario}
            onChange={(scenario) => dispatch({ type: "scenario", scenario })}
          />
          <Segmented
            label="Animation"
            options={[
              { value: "animate", label: "Animate" },
              { value: "step", label: "Step by hand" },
            ]}
            value={mode}
            onChange={setMode}
          />
        </div>

        {/* The two worlds: committed and candidate. */}
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <div
            className={cn(
              "rounded-2xl border-2 border-green-500/40 bg-green-500/[0.04] p-4 transition-[box-shadow,border-color] duration-500",
              state.status === "published" && "border-green-400/80 shadow-[0_0_32px_-6px_rgba(74,222,128,0.45)]",
              state.status === "stopped" && "border-rose-400/50",
            )}
          >
            <p className="text-[10px] font-black tracking-[0.2em] text-green-400/90 uppercase">Committed world</p>
            <p className="mt-2 font-mono text-2xl text-white tabular-nums">tick {grouped(state.tick)}</p>
            <p className="font-mono text-xs text-slate-400 tabular-nums">epoch {grouped(state.epoch)}</p>
            <ul className="mt-3 flex flex-wrap gap-1.5">
              {COMMITTED.map((part) => (
                <li key={part} className="rounded-md border border-green-500/25 bg-black/30 px-2 py-0.5 text-[11px] text-green-200">
                  {part}
                </li>
              ))}
            </ul>
            <p className="mt-3 text-xs leading-relaxed text-slate-400">
              {state.status === "stopped"
                ? "Kept bit for bit. The run has stopped; nothing more is prepared."
                : "Authoritative. Observers, the inspector and checkpoints read only this."}
            </p>
          </div>

          <div
            className={cn(
              "rounded-2xl border-2 border-dashed p-4 transition-colors duration-300",
              preparing ? "border-cyan-300/50 bg-cyan-300/[0.04]" : "border-white/10 bg-black/20",
              state.status === "cancelled" && "border-slate-500/40",
              state.status === "paused" && "border-amber-300/50",
              state.status === "stopped" && "border-rose-400/50",
            )}
          >
            <p className="text-[10px] font-black tracking-[0.2em] text-cyan-200/90 uppercase">Candidate</p>
            <p className={cn("mt-2 font-mono text-2xl tabular-nums", preparing ? "text-white" : "text-slate-500")}>
              {state.status === "published" ? "published" : `tick ${grouped(state.tick + 1)}`}
            </p>
            <p className="font-mono text-xs text-slate-400">
              {preparing && `phase ${state.phase} of ${PHASES}`}
              {state.status === "idle" && "none yet"}
              {state.status === "published" && `now tick ${grouped(state.tick)}`}
              {state.status === "cancelled" && `discarded in phase ${state.at}`}
              {state.status === "paused" && `paused in phase ${state.at}: pause-before-commit`}
              {state.status === "stopped" && `faulted in phase ${state.at}: stop-and-preserve`}
            </p>
            <ol className="mt-3 grid grid-cols-10 gap-1" aria-hidden="true">
              {tickPhases.map((phase, index) => {
                const number = index + 1;
                const current = preparing && number === state.phase;
                const finished = number <= done;
                const failed = ended && number === state.at;
                return (
                  <li
                    key={phase}
                    className={cn(
                      "h-2 rounded-full transition-colors duration-300",
                      finished ? (state.status === "published" ? "bg-green-400" : "bg-cyan-300/70") : "bg-white/10",
                      current && "animate-pulse bg-cyan-200",
                      failed && state.status === "cancelled" && "bg-slate-400",
                      failed && state.status === "paused" && "bg-amber-300",
                      failed && state.status === "stopped" && "bg-rose-400",
                      ended && number < state.at && "bg-slate-600",
                    )}
                  />
                );
              })}
            </ol>
            <ul className="mt-3 flex flex-wrap gap-1.5">
              {SCRATCH.map((part) => {
                const written = preparing ? state.phase >= part.from : false;
                return (
                  <li
                    key={part.label}
                    className={cn(
                      "rounded-md border px-2 py-0.5 text-[11px] transition-colors duration-300",
                      written ? "border-cyan-300/40 bg-cyan-300/10 text-cyan-100" : "border-white/10 text-slate-500",
                    )}
                  >
                    {part.label}
                  </li>
                );
              })}
            </ul>
            <p className="mt-3 text-xs leading-relaxed text-slate-400">
              {ended
                ? "Everything it wrote is dropped with it. Its staged genomes are freed; their charges never reached the world's quota."
                : "The only things a candidate writes. None of them is the committed world."}
            </p>
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <DemoButton onClick={() => dispatch({ type: "start" })} disabled={!canStart(state)}>
            <Play className="h-3.5 w-3.5" aria-hidden="true" />
            {state.status === "paused" ? "Retry the tick" : "Run a tick"}
          </DemoButton>
          <DemoButton onClick={() => dispatch({ type: "advance" })} disabled={!preparing}>
            <SkipForward className="h-3.5 w-3.5" aria-hidden="true" />
            {state.phase === PHASES ? "Check and publish" : "Next phase"}
          </DemoButton>
          <DemoButton
            onClick={() => dispatch({ type: "cancel" })}
            disabled={!canCancel(state)}
            className="hover:border-rose-400/60 hover:text-rose-300"
          >
            <Ban className="h-3.5 w-3.5" aria-hidden="true" /> Cancel
          </DemoButton>
          <DemoButton onClick={() => dispatch({ type: "reset", tick: START_TICK })} label="Reset the diagram">
            <RotateCcw className="h-3.5 w-3.5" aria-hidden="true" /> Reset
          </DemoButton>
        </div>

        <p
          aria-live="polite"
          className={cn(
            "flex min-h-[3rem] items-start gap-2 rounded-xl border px-3 py-2.5 text-sm leading-relaxed",
            state.status === "published" && "border-green-500/30 bg-green-500/5 text-green-100",
            state.status === "cancelled" && "border-white/15 bg-white/5 text-slate-200",
            state.status === "paused" && "border-amber-300/30 bg-amber-300/5 text-amber-100",
            state.status === "stopped" && "border-rose-400/30 bg-rose-400/5 text-rose-100",
            (state.status === "idle" || preparing) && "border-white/10 bg-black/20 text-slate-300",
          )}
        >
          {state.status === "published" && <Check className="mt-1 h-4 w-4 shrink-0" aria-hidden="true" />}
          {state.status === "paused" && <CirclePause className="mt-1 h-4 w-4 shrink-0" aria-hidden="true" />}
          {state.status === "stopped" && <OctagonX className="mt-1 h-4 w-4 shrink-0" aria-hidden="true" />}
          <span>{message(state)}</span>
        </p>

        {/* The ten phases, and what each writes. */}
        <ol className="space-y-1.5">
          {tickPhases.map((phase, index) => {
            const number = index + 1;
            const current = preparing && number === state.phase;
            const finished = number <= done;
            const stoppedHere = ended && number === state.at;
            const hazard =
              (state.scenario === "capacity" && number === PAUSE_PHASE) || (state.scenario === "fault" && number === FAULT_PHASE);
            return (
              <li
                key={phase}
                aria-current={current ? "step" : undefined}
                className={cn(
                  "flex items-start gap-3 rounded-xl border px-3 py-2.5 transition-colors duration-300 sm:gap-4 sm:px-4",
                  current ? "border-cyan-300/50 bg-cyan-300/[0.06]" : "border-white/[0.06] bg-black/20",
                  stoppedHere && state.status === "paused" && "border-amber-300/50 bg-amber-300/[0.06]",
                  stoppedHere && state.status === "stopped" && "border-rose-400/50 bg-rose-400/[0.06]",
                  stoppedHere && state.status === "cancelled" && "border-slate-400/40 bg-white/[0.04]",
                )}
              >
                <span
                  className={cn(
                    "w-6 shrink-0 pt-px font-mono text-sm font-black tabular-nums",
                    finished ? "text-green-400" : current ? "text-cyan-200" : "text-slate-500",
                  )}
                >
                  {String(number).padStart(2, "0")}
                </span>
                <span className="min-w-0 flex-1">
                  <span className={cn("block text-sm leading-snug", current || finished ? "text-slate-100" : "text-slate-300")}>
                    {phase}
                  </span>
                  <span className="mt-0.5 block font-mono text-[11px] leading-snug text-slate-500">{WRITES[index]}</span>
                </span>
                {hazard && !preparing && state.status !== "stopped" && state.status !== "paused" ? (
                  <span
                    className={cn(
                      "shrink-0 rounded-full border px-2 py-0.5 text-[9px] font-black tracking-[0.12em] uppercase",
                      state.scenario === "fault" ? "border-rose-400/30 text-rose-200" : "border-amber-300/30 text-amber-200",
                    )}
                  >
                    {state.scenario === "fault" ? "fault here" : "shortfall here"}
                  </span>
                ) : null}
                {finished ? <Check className="mt-0.5 h-4 w-4 shrink-0 text-green-400/80" aria-hidden="true" /> : null}
              </li>
            );
          })}
        </ol>
      </div>
    </DemoFrame>
  );
}
