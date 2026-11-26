"use client";

import { Equal, EqualNot, GitBranch, RotateCcw, X, Zap } from "lucide-react";
import { type MouseEvent, type ReactNode, useState } from "react";

import { DemoFrame, Slider } from "@/components/explain/demo";
import {
  END,
  INTERVAL,
  type LaneId,
  type Lanes,
  digestHue,
  newestCheckpoint,
  toyDigest,
  worldAt,
} from "@/components/explain/branch-model";
import { cn, grouped } from "@/lib/utils";

const at = (tick: number) => `${(tick / END) * 100}%`;
const CHECKPOINTS = Array.from({ length: END / INTERVAL + 1 }, (_, index) => index * INTERVAL);
const AXIS = [0, 1024, 2048, 3072, 4096];

const LANES: { id: LaneId; name: string; note: string }[] = [
  { id: "run", name: "The run", note: "uninterrupted, checkpointed" },
  { id: "resumed", name: "Killed, resumed", note: "same config, SIGKILLed" },
  { id: "control", name: "Control branch", note: "no command of its own" },
  { id: "brushed", name: "Intervention", note: "one energy brush" },
];

/** A thin horizontal segment of a lane, from tick `from` to `to`. */
function Segment({ from, to, className, top = "50%" }: { from: number; to: number; className: string; top?: string }) {
  if (to <= from) return null;
  return (
    <span
      aria-hidden="true"
      className={cn("absolute h-[3px] -translate-y-1/2 rounded-full", className)}
      style={{ left: at(from), width: `${((to - from) / END) * 100}%`, top }}
    />
  );
}

/** A marker centred on tick `tick`. */
function Marker({ tick, children, className, top = "50%" }: { tick: number; children: ReactNode; className?: string; top?: string }) {
  return (
    <span
      aria-hidden="true"
      className={cn("absolute flex -translate-x-1/2 -translate-y-1/2 items-center justify-center", className)}
      style={{ left: at(tick), top }}
    >
      {children}
    </span>
  );
}

function Digest({ digest }: { digest: string | null }) {
  if (digest === null) return <span className="font-mono text-xs text-slate-500">no world yet</span>;
  return (
    <span className="inline-flex items-center gap-2 font-mono text-xs text-slate-200 tabular-nums">
      <span
        aria-hidden="true"
        className="h-3 w-3 shrink-0 rounded-sm"
        style={{ background: `hsl(${digestHue(digest)} 70% 58%)` }}
      />
      {digest}
    </span>
  );
}

/**
 * Determinism you can see: a kept run, the same run killed and resumed, a
 * control branch and an intervention branch, compared tick by tick (README
 * FL-037, FL-039). The digests are toy hashes that are equal exactly where
 * flife's promise makes the worlds equal.
 */
export function BranchDiagram() {
  const [lanes, setLanes] = useState<Lanes>({ branchAt: 1536, brushAt: 2304, killAt: 3200 });
  const [compare, setCompare] = useState(3584);
  const resumeFrom = newestCheckpoint(lanes.killAt);

  const setBranch = (branchAt: number) =>
    setLanes((current) => ({ ...current, branchAt, brushAt: Math.max(current.brushAt, branchAt) }));
  // A click on any track moves the comparison there.
  const pick = (event: MouseEvent<HTMLDivElement>) => {
    const box = event.currentTarget.getBoundingClientRect();
    const tick = Math.round((((event.clientX - box.left) / box.width) * END) / 32) * 32;
    setCompare(Math.min(END, Math.max(0, tick)));
  };

  const runDigest = toyDigest("run", compare);
  const rows = LANES.map((lane) => {
    const world = worldAt(lane.id, compare, lanes);
    const digest = world === null ? null : toyDigest(world, compare);
    return { ...lane, digest, same: digest === runDigest };
  });

  const track = (lane: LaneId) => {
    switch (lane) {
      case "run":
        return (
          <>
            <Segment from={0} to={END} className="bg-green-400/80" />
            {CHECKPOINTS.map((tick) => (
              <Marker key={tick} tick={tick}>
                <span className="h-2 w-2 rotate-45 border border-green-300 bg-[#020a02]" />
              </Marker>
            ))}
          </>
        );
      case "resumed":
        return (
          <>
            <Segment from={0} to={lanes.killAt} className="bg-slate-500/70" top="32%" />
            <Marker tick={lanes.killAt} top="32%">
              <X className="h-3.5 w-3.5 text-rose-400" strokeWidth={3} />
            </Marker>
            <Segment from={resumeFrom} to={END} className="bg-green-400/80" top="70%" />
            <Marker tick={resumeFrom} top="70%">
              <RotateCcw className="h-3 w-3 text-green-300" strokeWidth={3} />
            </Marker>
          </>
        );
      case "control":
        return (
          <>
            <Segment from={lanes.branchAt} to={END} className="bg-green-400/80" />
            <Marker tick={lanes.branchAt}>
              <GitBranch className="h-3.5 w-3.5 text-green-300" strokeWidth={2.5} />
            </Marker>
          </>
        );
      case "brushed":
        return (
          <>
            <Segment from={lanes.branchAt} to={lanes.brushAt} className="bg-green-400/80" />
            <Segment from={lanes.brushAt} to={END} className="bg-rose-400/80" />
            <Marker tick={lanes.branchAt}>
              <GitBranch className="h-3.5 w-3.5 text-green-300" strokeWidth={2.5} />
            </Marker>
            <Marker tick={lanes.brushAt}>
              <Zap className="h-3.5 w-3.5 fill-amber-300 text-amber-300" />
            </Marker>
          </>
        );
    }
  };

  const commands = [
    `flife run CONFIG --ticks ${END} --run-dir run`,
    `flife run CONFIG --ticks ${END} --run-dir killed`,
    `# SIGKILLed at tick ${lanes.killAt}; it resumes from its checkpoint at ${resumeFrom}`,
    `flife resume killed --until ${END}`,
    `flife branch run control --at ${lanes.branchAt}`,
    `flife branch run brushed --at ${lanes.branchAt} --inject ${lanes.brushAt}:64,64:256`,
    `flife resume control --until ${END}`,
    `flife resume brushed --until ${END}`,
  ];

  return (
    <DemoFrame
      eyebrow="Run directories · FL-037, FL-039"
      title="Same commands, same world. Change one, and it differs only after."
      basis="diagram"
      source={
        <span className="[overflow-wrap:anywhere]">
          README.md (FL-037, FL-039); crates/flife-cli/tests/resume.rs
          (a_killed_write_ahead_run_resumes_to_the_uninterrupted_state) and tests/branch.rs
          (a_control_branch_continues_as_its_parent_while_an_intervention_branch_diverges). The digests here are toy
          32-bit hashes of each lane&apos;s world, not flife&apos;s SHA-256 world digests; checkpoints every{" "}
          {INTERVAL} ticks are this diagram&apos;s, a preset sets its own interval.
        </span>
      }
    >
      <div className="space-y-5">
        <div className="grid grid-cols-1 gap-x-6 gap-y-4 sm:grid-cols-2">
          <Slider
            label="Branch at checkpoint"
            value={lanes.branchAt}
            min={INTERVAL}
            max={END - INTERVAL}
            step={INTERVAL}
            onChange={setBranch}
            display={`tick ${grouped(lanes.branchAt)}`}
          />
          <Slider
            label="The brush applies at"
            value={lanes.brushAt}
            min={lanes.branchAt}
            max={END - 64}
            step={64}
            onChange={(brushAt) => setLanes((current) => ({ ...current, brushAt }))}
            display={`tick ${grouped(lanes.brushAt)}`}
          />
          <Slider
            label="SIGKILL the second run at"
            value={lanes.killAt}
            min={64}
            max={END - 64}
            step={64}
            onChange={(killAt) => setLanes((current) => ({ ...current, killAt }))}
            display={`tick ${grouped(lanes.killAt)}`}
          />
          <Slider
            label="Compare the worlds at"
            value={compare}
            min={0}
            max={END}
            step={32}
            onChange={setCompare}
            display={`tick ${grouped(compare)}`}
          />
        </div>

        {/* The lanes on one time axis. */}
        <div className="rounded-2xl border border-white/10 bg-black/30 p-3 sm:p-4">
          <div className="grid grid-cols-[5.75rem_minmax(0,1fr)] items-center gap-x-3 gap-y-2 sm:grid-cols-[8.5rem_minmax(0,1fr)]">
            {LANES.map((lane) => (
              <div key={lane.id} className="contents">
                <div className="min-w-0">
                  <p className="truncate text-xs font-bold text-slate-200">{lane.name}</p>
                  <p className="hidden truncate text-[10px] text-slate-500 sm:block">{lane.note}</p>
                </div>
                <div className="relative h-9 cursor-crosshair" onClick={pick} aria-hidden="true">
                  <span className="absolute inset-x-0 top-1/2 h-px -translate-y-1/2 bg-white/[0.06]" />
                  {track(lane.id)}
                  <span className="absolute top-0 bottom-0 w-px bg-cyan-200/70" style={{ left: at(compare) }} />
                </div>
              </div>
            ))}
            <div />
            {/* Phones label every other tick: at 320 px the five labels shared 116 px and ran together. */}
            <div className="relative h-4" aria-hidden="true">
              {AXIS.map((tick, index) => (
                <span
                  key={tick}
                  className={cn(
                    "absolute top-0 -translate-x-1/2 font-mono text-[10px] text-slate-500 tabular-nums first:translate-x-0 last:-translate-x-full",
                    index % 2 === 1 && "max-sm:hidden",
                  )}
                  style={{ left: at(tick) }}
                >
                  {grouped(tick)}
                </span>
              ))}
            </div>
          </div>
          <ul className="mt-3 flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-slate-400">
            <li className="inline-flex items-center gap-1.5">
              <span className="h-2 w-2 rotate-45 border border-green-300" /> checkpoint
            </li>
            <li className="inline-flex items-center gap-1.5">
              <X className="h-3 w-3 text-rose-400" strokeWidth={3} aria-hidden="true" /> killed
            </li>
            <li className="inline-flex items-center gap-1.5">
              <RotateCcw className="h-3 w-3 text-green-300" strokeWidth={3} aria-hidden="true" /> resumed from the newest
              checkpoint
            </li>
            <li className="inline-flex items-center gap-1.5">
              <GitBranch className="h-3 w-3 text-green-300" aria-hidden="true" /> branched
            </li>
            <li className="inline-flex items-center gap-1.5">
              <Zap className="h-3 w-3 fill-amber-300 text-amber-300" aria-hidden="true" /> brush applies
            </li>
          </ul>
        </div>

        {/* Each lane's world at the compared tick. */}
        <div aria-live="polite">
          <p className="mb-2 text-xs text-slate-400">
            The worlds at tick <span className="font-mono text-white">{grouped(compare)}</span> (toy digests):
          </p>
          <table className="w-full text-left text-sm">
            <tbody>
              {rows.map((row) => (
                <tr key={row.id} className="border-t border-white/[0.06]">
                  <th scope="row" className="py-2 pr-3 text-xs font-bold text-slate-300">
                    {row.name}
                  </th>
                  <td className="py-2 pr-3">
                    <Digest digest={row.digest} />
                  </td>
                  <td className="py-2 text-right text-xs">
                    {row.id === "run" ? (
                      <span className="text-slate-500">reference</span>
                    ) : row.digest === null ? (
                      <span className="text-slate-500">branches at {grouped(lanes.branchAt)}</span>
                    ) : row.same ? (
                      <span className="inline-flex items-center gap-1 font-bold text-green-300">
                        <Equal className="h-3.5 w-3.5" aria-hidden="true" /> the run&apos;s world
                      </span>
                    ) : (
                      <span className="inline-flex items-center gap-1 font-bold text-rose-300">
                        <EqualNot className="h-3.5 w-3.5" aria-hidden="true" /> differs
                      </span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <p className="mt-3 text-xs leading-relaxed text-slate-400">
            Killed part way, a run resumes from its newest restorable checkpoint and reaches exactly the uninterrupted
            run&apos;s world and final checkpoint bytes, across controller phases, cooldowns, newborns and commands still in the
            future. A control branch reaches the parent&apos;s world; its intervention sibling matches it until
            the brush applies at tick {grouped(lanes.brushAt)}&apos;s boundary, and differs from tick{" "}
            {grouped(lanes.brushAt + 1)} on. A branch links the parent&apos;s verified chunk files instead of copying
            them.
          </p>
        </div>

        {/* Focusable, so a keyboard can scroll the long lines sideways (axe: scrollable-region-focusable). */}
        <pre tabIndex={0} className="overflow-x-auto rounded-xl border border-white/10 bg-black/40 p-3 text-[12px] leading-relaxed text-green-200 [scrollbar-width:thin]">
          <code>{commands.join("\n")}</code>
        </pre>
      </div>
    </DemoFrame>
  );
}
