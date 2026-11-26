"use client";

// The v2 mechanisms, told as two step-throughs (the tissue's own, then the
// supercolony's), a row of rules both worlds share and a row of opt-ins. Each
// chart is its rule computed in the page from v2-atlas-math.ts at the
// committed presets' numbers; the words and numbers are lib/v2-atlas.ts's,
// whose sources it names, and each still is lib/stills.json's.

import { ArrowLeft, ArrowRight } from "lucide-react";
import { type KeyboardEvent, type ReactNode, useId, useRef, useState } from "react";

import { MicroLabel, Reticle } from "@/components/franken";
import { type V2Still, stills } from "@/lib/media";
import { TONE } from "@/lib/palette";
import { cn, grouped } from "@/lib/utils";
import { type AtlasChart, type AtlasEntry, atlas, atlasEntry, storySteps } from "@/lib/v2-atlas";

import {
  affinityTurn,
  BARRIER,
  cadherinAdhesion,
  DECAY,
  emissionSurplus,
  epigeneticGate,
  halfLife,
  HIPPO,
  hippoGate,
  kinWeight,
  OASES,
  oasisPowerShare,
  permeability,
  readinessTrace,
  senescenceCapAge,
  senescenceMultiplier,
  steadyConductance,
  SURPLUS_FLOOR,
  VASCULAR,
} from "./v2-atlas-math";

// ─── A small plot: 240 × 132 units, axes at the left and bottom ──────────────

const W = 240;
const H = 132;
// A plotted chart is taller than a diagram: its y-axis label sits above the plot, clear of the data.
const FH = 144;
const PAD = { left: 26, right: 8, top: 20, bottom: 22 };
const IW = W - PAD.left - PAD.right;
const IH = FH - PAD.top - PAD.bottom;
/** A dark outline under a label, so a line that crosses it breaks around the letters (fixture-panels.tsx draws
 *  its 11-unit labels with a 4-unit one; these are 7 to 8 units). */
const HALO = { stroke: TONE.ink, strokeWidth: 3, strokeLinejoin: "round", paintOrder: "stroke" } as const;

type Scale = { x: (value: number) => number; y: (value: number) => number };

function scale(xMax: number, yMax: number, xMin = 0, yMin = 0): Scale {
  return {
    x: (value) => PAD.left + ((value - xMin) / (xMax - xMin)) * IW,
    y: (value) => PAD.top + IH - ((value - yMin) / (yMax - yMin)) * IH,
  };
}

function line(points: [number, number][], s: Scale): string {
  return points.map(([x, y], at) => `${at === 0 ? "M" : "L"}${s.x(x).toFixed(1)},${s.y(y).toFixed(1)}`).join(" ");
}

function sample(from: number, to: number, count: number, f: (x: number) => number): [number, number][] {
  return Array.from({ length: count + 1 }, (_, at) => {
    const x = from + ((to - from) * at) / count;
    return [x, f(x)] as [number, number];
  });
}

function Frame({ title, children, xLabel, yLabel, xTicks, yTicks, s }: {
  title: string;
  children: ReactNode;
  xLabel: string;
  yLabel: string;
  xTicks: { at: number; label: string }[];
  yTicks: { at: number; label: string }[];
  s: Scale;
}) {
  return (
    <svg viewBox={`0 0 ${W} ${FH}`} role="img" aria-label={title} className="h-auto w-full overflow-visible">
      <g stroke="rgba(255,255,255,0.14)" strokeWidth="1">
        <line x1={PAD.left} y1={PAD.top + IH} x2={PAD.left + IW} y2={PAD.top + IH} />
        <line x1={PAD.left} y1={PAD.top} x2={PAD.left} y2={PAD.top + IH} />
      </g>
      <g fill="#94a3b8" fontSize="7.5" fontFamily="var(--font-mono, ui-monospace)">
        {xTicks.map((tick) => (
          <text key={`x${tick.at}`} x={s.x(tick.at)} y={PAD.top + IH + 10} textAnchor="middle">
            {tick.label}
          </text>
        ))}
        {yTicks.map((tick) => (
          <text key={`y${tick.at}`} x={PAD.left - 4} y={s.y(tick.at) + 2.5} textAnchor="end">
            {tick.label}
          </text>
        ))}
        <text x={PAD.left + IW} y={FH - 1} textAnchor="end" fill="#64748b">
          {xLabel}
        </text>
        <text x={PAD.left - 4} y={8} fill="#64748b">
          {yLabel}
        </text>
      </g>
      {children}
    </svg>
  );
}

function Dot({ x, y, s, colour = TONE.rule, label, anchor = "start", below = false }: {
  x: number;
  y: number;
  s: Scale;
  colour?: string;
  label?: string;
  anchor?: "start" | "end" | "middle";
  /** Put the label under the point, for a curve that rises past it. */
  below?: boolean;
}) {
  const dx = anchor === "start" ? 5 : anchor === "end" ? -5 : 0;
  return (
    <g>
      <circle cx={s.x(x)} cy={s.y(y)} r="2.4" fill={colour} />
      {label ? (
        <text x={s.x(x) + dx} y={s.y(y) + (below ? 10 : -4)} fontSize="7.5" fill="#e2e8f0" textAnchor={anchor} fontFamily="var(--font-mono, ui-monospace)" {...HALO}>
          {label}
        </text>
      ) : null}
    </g>
  );
}

function Bars({ values, s, colour = TONE.rule, labels }: { values: number[]; s: Scale; colour?: string; labels?: string[] }) {
  const width = (IW / values.length) * 0.56;
  return (
    <g>
      {values.map((value, at) => {
        const x = s.x(at) - width / 2;
        return (
          <g key={at}>
            <rect x={x} y={s.y(value)} width={width} height={Math.max(0.5, s.y(0) - s.y(value))} rx="1.5" fill={colour} opacity={0.85} />
            {labels?.[at] ? (
              <text x={s.x(at)} y={s.y(value) - 3} fontSize="7" textAnchor="middle" fill="#e2e8f0" fontFamily="var(--font-mono, ui-monospace)">
                {labels[at]}
              </text>
            ) : null}
          </g>
        );
      })}
    </g>
  );
}

// ─── The charts ──────────────────────────────────────────────────────────────

function OasesChart() {
  const s = scale(24, 3.6);
  const shown = [0, 4, 8, 12];
  const colours = ["#2EC4B6", "#9B5DE5", "#FFB627", "#F15BB5"];
  return (
    <Frame
      title="Four of the sixteen sources' power over one 24-second cycle, as multiples of a source's average share; the total stays constant"
      s={s}
      xLabel="seconds"
      yLabel="× share"
      xTicks={[0, 6, 12, 18, 24].map((at) => ({ at, label: String(at) }))}
      yTicks={[0, 1, 2, 3].map((at) => ({ at, label: String(at) }))}
    >
      <path d={line([[0, 1], [24, 1]], s)} stroke="#e2e8f0" strokeDasharray="3 3" strokeWidth="1" fill="none" opacity="0.7" />
      {shown.map((j, at) => (
        <path key={j} d={line(sample(0, 24, 120, (t) => oasisPowerShare(t, j)), s)} stroke={colours[at]} strokeWidth="1.6" fill="none" />
      ))}
      {/* Some source crosses the mean every few seconds, so no stretch of it is free for the label: a halo instead. */}
      <text x={s.x(23.5)} y={s.y(1) - 3} fontSize="7" fill="#e2e8f0" textAnchor="end" fontFamily="var(--font-mono, ui-monospace)" {...HALO}>
        mean of all {OASES.sources}
      </text>
    </Frame>
  );
}

function SenescenceChart() {
  const s = scale(300, 66, 0, 0);
  const cap = senescenceCapAge();
  return (
    <Frame
      title="Maintenance multiplier against age: 2 at 30 s, 5 at 60 s, 64 from about 238 s"
      s={s}
      xLabel="age, s"
      yLabel="× upkeep"
      xTicks={[0, 60, 120, 180, 240, 300].map((at) => ({ at, label: String(at) }))}
      yTicks={[1, 16, 32, 48, 64].map((at) => ({ at, label: String(at) }))}
    >
      <path d={line(sample(0, 300, 150, (a) => senescenceMultiplier(a)), s)} stroke={TONE.energy} strokeWidth="1.8" fill="none" />
      {/* 21 units apart: room above and left of each point, clear of the curve, for "2×" and "5×" alone (the
          caption gives their ages); "2× at 30 s" and "5× at 60 s" ran into each other at every width. */}
      <Dot x={30} y={2} s={s} colour={TONE.energy} label="2×" anchor="end" />
      <Dot x={60} y={5} s={s} colour={TONE.energy} label="5×" anchor="end" />
      <Dot x={cap} y={64} s={s} colour={TONE.energy} label={`cap at ${cap.toFixed(0)} s`} anchor="end" />
    </Frame>
  );
}

function SurplusChart() {
  const s = scale(2, 1.6);
  return (
    <Frame
      title="Energy emission may spend against the organism's energy: nothing up to 0.45, then everything above it"
      s={s}
      xLabel="energy E"
      yLabel="spendable"
      xTicks={[0, 0.45, 1, 1.5, 2].map((at) => ({ at, label: String(at) }))}
      yTicks={[0, 0.5, 1, 1.5].map((at) => ({ at, label: String(at) }))}
    >
      <rect x={s.x(0)} y={PAD.top} width={s.x(SURPLUS_FLOOR) - s.x(0)} height={IH} fill={TONE.wrong} opacity="0.08" />
      {/* The floorless line stops at the top of the plot (1.6), not at E = 2 above the frame. */}
      <path d={line(sample(0, 1.6, 64, (e) => e), s)} stroke="#64748b" strokeDasharray="3 3" strokeWidth="1" fill="none" />
      <path d={line(sample(0, 2, 80, (e) => emissionSurplus(e)), s)} stroke={TONE.rule} strokeWidth="1.8" fill="none" />
      <text x={s.x(0.04)} y={s.y(1.42)} fontSize="7" fill="#fda4af" fontFamily="var(--font-mono, ui-monospace)">
        protected
      </text>
      {/* Left of and above its own line, past the protected band: at the top right, both lines ran through it. */}
      <text x={s.x(1.2)} y={s.y(1.36)} fontSize="7" fill="#64748b" textAnchor="end" fontFamily="var(--font-mono, ui-monospace)">
        without the floor
      </text>
    </Frame>
  );
}

function RelayChart() {
  const ticks = 128;
  const { readiness, fired } = readinessTrace(ticks);
  const s = scale(ticks / 32, 1);
  return (
    <Frame
      title="Readiness of an organism that always wants to emit: it bursts each time readiness recovers past one half, every 21 ticks, and each burst drains 85% of it"
      s={s}
      xLabel="seconds"
      yLabel="readiness"
      xTicks={[0, 1, 2, 3, 4].map((at) => ({ at, label: String(at) }))}
      yTicks={[0, 0.5, 1].map((at) => ({ at, label: String(at) }))}
    >
      <path d={line([[0, 0.5], [4, 0.5]], s)} stroke="#64748b" strokeDasharray="3 3" strokeWidth="1" fill="none" />
      {fired.map((tick) => (
        <line key={tick} x1={s.x(tick / 32)} x2={s.x(tick / 32)} y1={s.y(0)} y2={s.y(0.92)} stroke={TONE.chemistry} strokeWidth="1.4" opacity="0.75" />
      ))}
      <path d={line(readiness.map((r, tick) => [tick / 32, r] as [number, number]), s)} stroke={TONE.energy} strokeWidth="1.6" fill="none" />
      <text x={s.x(fired[1] !== undefined ? fired[1] / 32 : 1) + 3} y={s.y(0.97)} fontSize="7" fill={TONE.chemistry} fontFamily="var(--font-mono, ui-monospace)">
        bursts
      </text>
    </Frame>
  );
}

function JunctionsChart() {
  const values = [0, 1, 2, 3, 4, 5].map((n) => permeability(n));
  const s = scale(5.5, 1.1, -0.5, 0);
  return (
    <Frame
      title="Permeability of a field cell against the organisms in it: 1, 0.48, 0.32 and falling toward 0.04"
      s={s}
      xLabel="organisms in the cell"
      yLabel="lets through"
      xTicks={[0, 1, 2, 3, 4, 5].map((at) => ({ at, label: String(at) }))}
      yTicks={[0, 0.5, 1].map((at) => ({ at, label: String(at) }))}
    >
      <path d={line([[-0.5, BARRIER.minimum], [5.5, BARRIER.minimum]], s)} stroke="#64748b" strokeDasharray="3 3" strokeWidth="1" fill="none" />
      <Bars values={values} s={s} colour={TONE.chemistry} labels={values.map((v) => `${Math.round(v * 100)}%`)} />
    </Frame>
  );
}

function VascularChart() {
  const s = scale(1, 1);
  const marks = [VASCULAR.halfSaturation];
  return (
    <Frame
      title="The conductance a face settles at under a steady energy flux: 0.025 with none, 0.72 at the half-saturation flux, near 0.84 at most"
      s={s}
      xLabel="steady flux |F|"
      yLabel="conductance g"
      xTicks={[0, 0.25, 0.5, 0.75, 1].map((at) => ({ at, label: String(at) }))}
      yTicks={[0, 0.5, 1].map((at) => ({ at, label: String(at) }))}
    >
      <path d={line(sample(0, 1, 120, (f) => steadyConductance(f)), s)} stroke="#f43f5e" strokeWidth="1.8" fill="none" />
      {marks.map((flux) => (
        <Dot key={flux} x={flux} y={steadyConductance(flux)} s={s} colour="#fda4af" label={`${steadyConductance(flux).toFixed(2)} at J₀`} below />
      ))}
      <Dot x={0} y={VASCULAR.minimum} s={s} colour="#fda4af" label="g_min" />
    </Frame>
  );
}

function TrailsChart() {
  const s = scale(60, 1);
  return (
    <Frame
      title={`What is left of a deposit over a minute: food halves in ${halfLife(DECAY.food).toFixed(1)} s, trail in ${halfLife(DECAY.trail).toFixed(0)} s`}
      s={s}
      xLabel="seconds"
      yLabel="left"
      xTicks={[0, 15, 30, 45, 60].map((at) => ({ at, label: String(at) }))}
      yTicks={[0, 0.5, 1].map((at) => ({ at, label: String(at) }))}
    >
      <path d={line(sample(0, 60, 120, (t) => Math.exp(-DECAY.food * t)), s)} stroke={TONE.chemistry} strokeWidth="1.6" fill="none" />
      <path d={line(sample(0, 60, 120, (t) => Math.exp(-DECAY.trail * t)), s)} stroke="#FFB627" strokeWidth="1.8" fill="none" />
      <Dot x={halfLife(DECAY.food)} y={0.5} s={s} colour={TONE.chemistry} label="food" />
      <Dot x={halfLife(DECAY.trail)} y={0.5} s={s} colour="#FFB627" label="trail" />
    </Frame>
  );
}

function HippoChart() {
  const values = [0, 1, 2, 3, 4, 5].map((n) => hippoGate(n));
  const s = scale(5.5, 1.1, -0.5, 0);
  return (
    <Frame
      title={`Chance to divide, relative, against neighbours within reach: zero from four (c_max = ${HIPPO.arrest})`}
      s={s}
      xLabel="neighbours"
      yLabel="birth gate"
      xTicks={[0, 1, 2, 3, 4, 5].map((at) => ({ at, label: String(at) }))}
      yTicks={[0, 0.5, 1].map((at) => ({ at, label: String(at) }))}
    >
      <Bars values={values} s={s} colour={TONE.rule} labels={values.map((v) => v.toFixed(2))} />
    </Frame>
  );
}

function KinChart() {
  const s = scale(1, 1);
  return (
    <Frame
      title="Weight kinship gives a transfer against the cosine of two diets: nothing below 0.5, then cos⁴"
      s={s}
      xLabel="diet cosine"
      yLabel="kin weight"
      xTicks={[0, 0.5, 1].map((at) => ({ at, label: String(at) }))}
      yTicks={[0, 0.5, 1].map((at) => ({ at, label: String(at) }))}
    >
      <rect x={s.x(0)} y={PAD.top} width={s.x(0.5) - s.x(0)} height={IH} fill="#64748b" opacity="0.08" />
      <path d={line(sample(0.5, 1, 60, (c) => kinWeight(c)), s)} stroke={TONE.energy} strokeWidth="1.8" fill="none" />
      <path d={line([[0, 0], [0.5, 0]], s)} stroke={TONE.energy} strokeWidth="1.8" fill="none" />
      {/* Left of the point, over the flat part: on the right the rising curve ran through it. */}
      <Dot x={0.5} y={kinWeight(0.5)} s={s} colour={TONE.energy} label="0.06" anchor="end" />
      <text x={s.x(0.03)} y={s.y(0.9)} fontSize="7" fill="#94a3b8" fontFamily="var(--font-mono, ui-monospace)">
        not kin
      </text>
    </Frame>
  );
}

function CadherinChart() {
  const s = scale(180, 1);
  return (
    <Frame
      title="Adhesion between two bodies against the angle between their tags: full when aligned, none from 90 degrees"
      s={s}
      xLabel="angle between tags, °"
      yLabel="adhesion"
      xTicks={[0, 45, 90, 135, 180].map((at) => ({ at, label: String(at) }))}
      yTicks={[0, 0.5, 1].map((at) => ({ at, label: String(at) }))}
    >
      <path d={line(sample(0, 180, 90, (deg) => cadherinAdhesion(Math.cos((deg * Math.PI) / 180))), s)} stroke={TONE.rule} strokeWidth="1.8" fill="none" />
      <Dot x={60} y={0.25} s={s} label="¼ at 60°" />
    </Frame>
  );
}

function GateChart() {
  const s = scale(8, 1, -4, 0);
  return (
    <Frame
      title="How much of the state is rewritten per tick against the network's push z: 0.076 at rest, a half at z = 2.5"
      s={s}
      xLabel="push z"
      yLabel="gate g"
      xTicks={[-4, 0, 2.5, 8].map((at) => ({ at, label: String(at) }))}
      yTicks={[0, 0.5, 1].map((at) => ({ at, label: String(at) }))}
    >
      <path d={line(sample(-4, 8, 120, (z) => epigeneticGate(z)), s)} stroke="#C8B6FF" strokeWidth="1.8" fill="none" />
      {/* Left of the point, over the flat part: on the right the rising curve ran through it. */}
      <Dot x={0} y={epigeneticGate(0)} s={s} colour="#C8B6FF" label="0.076 at rest" anchor="end" />
    </Frame>
  );
}

/** A diet vector a, its right-angle turn Ja, and the effective diet at a chosen h₀. */
function TurnChart() {
  const [h0, setH0] = useState(0.5);
  const id = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const angle = affinityTurn(h0);
  // Left of the middle: "a, inherited diet" (about 82 units) ends inside the 240 then; centred, 12 were cut off.
  const c = { x: 100, y: 64 };
  const r = 46;
  const tip = (theta: number, scale = 1) => ({ x: c.x + Math.cos(theta) * r * scale, y: c.y - Math.sin(theta) * r * scale });
  const a = tip(0);
  const ja = tip(Math.PI / 2);
  const eff = tip(angle);
  return (
    <div>
      <svg viewBox={`0 0 ${W} ${H - 8}`} role="img" aria-label={`The diet turned by ${Math.round((angle * 180) / Math.PI)} degrees at h₀ = ${h0.toFixed(2)}`} className="h-auto w-full">
        <defs>
          <marker id={`${id}-arrow`} viewBox="0 0 10 10" refX="8" refY="5" markerWidth="5" markerHeight="5" orient="auto-start-reverse">
            <path d="M0,0 L10,5 L0,10 z" fill="context-stroke" />
          </marker>
        </defs>
        <circle cx={c.x} cy={c.y} r={r} fill="none" stroke="rgba(255,255,255,0.1)" />
        <line x1={c.x} y1={c.y} x2={a.x} y2={a.y} stroke="#94a3b8" strokeWidth="1.4" markerEnd={`url(#${id}-arrow)`} />
        <line x1={c.x} y1={c.y} x2={ja.x} y2={ja.y} stroke="#64748b" strokeWidth="1.2" strokeDasharray="3 2" markerEnd={`url(#${id}-arrow)`} />
        <line x1={c.x} y1={c.y} x2={eff.x} y2={eff.y} stroke="#C8B6FF" strokeWidth="2.2" markerEnd={`url(#${id}-arrow)`} />
        <g fontSize="8" fontFamily="var(--font-mono, ui-monospace)">
          <text x={a.x + 4} y={a.y + 3} fill="#94a3b8">
            a, inherited diet
          </text>
          {/* Up and to the left of its tip: on the right, the circle ran through it, and a_eff (always on the
              right: the turn is ±90° at most) would meet it at 90°. */}
          <text x={ja.x - 4} y={ja.y - 4} fill="#64748b" textAnchor="end">
            Ja
          </text>
          <text x={eff.x + (Math.cos(angle) >= 0 ? 4 : -4)} y={eff.y - 4} fill="#ddd6fe" textAnchor={Math.cos(angle) >= 0 ? "start" : "end"}>
            a_eff
          </text>
        </g>
      </svg>
      <label htmlFor={`${id}-h0`} className="mt-1 flex items-center gap-3 font-mono text-[11px] text-slate-400">
        <span className="shrink-0">h₀ = {h0.toFixed(2)}</span>
        <input
          id={`${id}-h0`}
          type="range"
          min={-1}
          max={1}
          step={0.01}
          value={h0}
          onChange={(event) => setH0(Number(event.target.value))}
          className="w-full accent-violet-300"
        />
        <span className="shrink-0">{Math.round((angle * 180) / Math.PI)}°</span>
      </label>
    </div>
  );
}

/** One organism, its diet a and where each kind of deposit points: corpse −a, waste Ja, wake behind its motion. */
function DepositChart({ slug }: { slug: AtlasEntry["slug"] }) {
  const id = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const c = { x: 112, y: 64 };
  const lit = (on: boolean) => (on ? 1 : 0.28);
  const isLysis = slug === "lysis-recycling";
  const isWaste = slug === "symplectic-waste";
  const isWake = slug === "wake-offset";
  return (
    <svg viewBox={`0 0 ${W} ${H - 8}`} role="img" aria-label="Where deposits point: the diet a, a corpse along minus a, waste along J a, and a wake behind the motion" className="h-auto w-full">
      <defs>
        <marker id={`${id}-arrow`} viewBox="0 0 10 10" refX="8" refY="5" markerWidth="5" markerHeight="5" orient="auto-start-reverse">
          <path d="M0,0 L10,5 L0,10 z" fill="context-stroke" />
        </marker>
      </defs>
      <g fontSize="8" fontFamily="var(--font-mono, ui-monospace)">
        <line x1={c.x} y1={c.y} x2={c.x + 52} y2={c.y} stroke="#94a3b8" strokeWidth="1.4" markerEnd={`url(#${id}-arrow)`} />
        <text x={c.x + 56} y={c.y + 3} fill="#94a3b8">
          a (diet)
        </text>
        <g opacity={lit(isLysis)}>
          <line x1={c.x} y1={c.y} x2={c.x - 52} y2={c.y} stroke={TONE.wrong} strokeWidth="1.8" markerEnd={`url(#${id}-arrow)`} />
          <text x={c.x - 56} y={c.y + 3} fill="#fda4af" textAnchor="end">
            corpse −a
          </text>
        </g>
        <g opacity={lit(isWaste)}>
          <line x1={c.x} y1={c.y} x2={c.x} y2={c.y - 46} stroke={TONE.rule} strokeWidth="1.8" markerEnd={`url(#${id}-arrow)`} />
          <text x={c.x + 4} y={c.y - 44} fill="#86efac">
            waste Ja ⟂ a
          </text>
        </g>
        <g opacity={lit(isWake)}>
          <line x1={c.x + 8} y1={c.y + 34} x2={c.x + 46} y2={c.y + 34} stroke="#e2e8f0" strokeWidth="1.2" markerEnd={`url(#${id}-arrow)`} />
          <text x={c.x + 50} y={c.y + 37} fill="#e2e8f0">
            moving
          </text>
          <circle cx={c.x - 6} cy={c.y + 34} r="3" fill={TONE.chemistry} />
          <text x={c.x - 12} y={c.y + 37} fill={TONE.chemistry} textAnchor="end">
            deposit δ behind
          </text>
        </g>
      </g>
      <circle cx={c.x} cy={c.y} r="7" fill="#2EC4B6" stroke="#020a02" strokeWidth="1.5" />
    </svg>
  );
}

/** The motor's four terms around one organism: food gradient, chemical gradient, contour and heading. */
function MotorChart() {
  const id = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const c = { x: 120, y: 66 };
  const arrows = [
    { dx: 50, dy: -22, colour: TONE.energy, label: "∇E food" },
    { dx: 30, dy: 36, colour: TONE.chemistry, label: "∇C chemistry" },
    { dx: -42, dy: -30, colour: "#C8B6FF", label: "∇⊥ contour" },
    { dx: -48, dy: 18, colour: TONE.rule, label: "ĥ heading" },
  ];
  return (
    <svg viewBox={`0 0 ${W} ${H - 8}`} role="img" aria-label="The four steering terms: food gradient, chemical gradient, contour and heading, each scaled by a network output" className="h-auto w-full">
      <defs>
        <marker id={`${id}-arrow`} viewBox="0 0 10 10" refX="8" refY="5" markerWidth="5" markerHeight="5" orient="auto-start-reverse">
          <path d="M0,0 L10,5 L0,10 z" fill="context-stroke" />
        </marker>
      </defs>
      {arrows.map((arrow) => (
        <g key={arrow.label} fontSize="8" fontFamily="var(--font-mono, ui-monospace)">
          <line x1={c.x} y1={c.y} x2={c.x + arrow.dx} y2={c.y + arrow.dy} stroke={arrow.colour} strokeWidth="1.8" markerEnd={`url(#${id}-arrow)`} />
          <text x={c.x + arrow.dx * 1.08 + (arrow.dx >= 0 ? 2 : -2)} y={c.y + arrow.dy * 1.08 + 3} fill={arrow.colour} textAnchor={arrow.dx >= 0 ? "start" : "end"}>
            {arrow.label}
          </text>
        </g>
      ))}
      <circle cx={c.x} cy={c.y} r="7" fill="#9B5DE5" stroke="#020a02" strokeWidth="1.5" />
    </svg>
  );
}

function Chart({ chart, slug }: { chart: AtlasChart; slug: AtlasEntry["slug"] }) {
  switch (chart) {
    case "oases":
      return <OasesChart />;
    case "senescence":
      return <SenescenceChart />;
    case "surplus":
      return <SurplusChart />;
    case "relay":
      return <RelayChart />;
    case "junctions":
      return <JunctionsChart />;
    case "vascular":
      return <VascularChart />;
    case "trails":
      return <TrailsChart />;
    case "hippo":
      return <HippoChart />;
    case "gate":
      return <GateChart />;
    case "turn":
      return (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <GateChart />
          <TurnChart />
        </div>
      );
    case "kin":
      return <KinChart />;
    case "cadherin":
      return <CadherinChart />;
    case "motor":
      return <MotorChart />;
    case "deposit":
      return <DepositChart slug={slug} />;
  }
}

// ─── The step-throughs ───────────────────────────────────────────────────────

const ACCENT = {
  tissue: { text: "text-violet-200", value: "text-violet-200", ring: "border-violet-400/50 bg-violet-400/10", dot: "bg-violet-300" },
  supercolony: { text: "text-cyan-200", value: "text-cyan-200", ring: "border-cyan-400/50 bg-cyan-400/10", dot: "bg-cyan-300" },
} as const;

const stillById = (id: string): V2Still => {
  const found = stills.stills.find((still) => still.id === id);
  if (!found) throw new Error(`lib/stills.json has no still ${id}`);
  return found;
};

function Presence({ entry }: { entry: AtlasEntry }) {
  const chip = "rounded-md border px-1.5 py-0.5 font-mono text-[10px] font-bold";
  if (!entry.on.tissue && !entry.on.supercolony) {
    return <span className={cn(chip, "border-white/15 text-slate-400")}>off in both presets</span>;
  }
  return (
    <>
      {entry.on.tissue ? <span className={cn(chip, "border-violet-400/30 bg-violet-400/10 text-violet-200")}>tissue</span> : null}
      {entry.on.supercolony ? (
        <span className={cn(chip, "border-cyan-400/30 bg-cyan-400/10 text-cyan-200")}>supercolony</span>
      ) : null}
    </>
  );
}

/** The rule as numbers: its chart, its formula, the presets' values and where the energy goes. */
function RulePanel({ entry, toggle }: { entry: AtlasEntry; toggle: boolean }) {
  return (
    <div className="flex flex-col gap-4 p-5 sm:p-7">
      {/* On phones, below the world/rule toggle (absolute, top right): beside it, at 390 px, the two ran into
          each other. */}
      <span
        className={cn(
          "self-start rounded-full border border-cyan-300/25 bg-cyan-300/[0.06] px-2.5 py-1 font-mono text-[10px] font-bold tracking-[0.12em] text-cyan-100 uppercase",
          toggle && "max-sm:mt-10",
        )}
      >
        The rule, computed here
      </span>
      {entry.chart ? (
        <div className="rounded-xl border border-white/[0.06] bg-black/35 p-3 sm:p-4">
          <Chart chart={entry.chart} slug={entry.slug} />
        </div>
      ) : null}
      {entry.formula ? (
        <p className="rounded-lg bg-black/40 px-3 py-2 font-mono text-[12px] break-words whitespace-pre-wrap text-green-200">{entry.formula}</p>
      ) : null}
      <dl className="space-y-2 text-[13px] leading-relaxed">
        <div className="flex gap-3">
          <dt className="w-[4.4rem] shrink-0 text-slate-500">Numbers</dt>
          <dd className="text-slate-300">{entry.numbers}</dd>
        </div>
        <div className="flex gap-3">
          <dt className="w-[4.4rem] shrink-0 text-slate-500">Energy</dt>
          <dd className="text-slate-300">{entry.ledger}</dd>
        </div>
      </dl>
    </div>
  );
}

/** One step's picture: the still drawn with the view that shows its state, or else its rule. */
function Stage({ entry, view, setView }: { entry: AtlasEntry; view: "world" | "rule"; setView: (view: "world" | "rule") => void }) {
  const still = entry.still ? stillById(entry.still) : null;
  const showStill = still !== null && view === "world";
  return (
    <figure className="panel relative overflow-hidden rounded-[1.75rem]">
      {still ? (
        <div role="group" aria-label="Show" className="absolute top-3 right-3 z-10 inline-flex rounded-full border border-white/10 bg-black/70 p-0.5 backdrop-blur-md">
          {(["world", "rule"] as const).map((option) => (
            <button
              key={option}
              type="button"
              aria-pressed={view === option}
              onClick={() => setView(option)}
              className={cn(
                "min-h-9 rounded-full px-3 text-[12px] font-bold transition-colors focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
                view === option ? "bg-white/15 text-white" : "text-slate-400 hover:text-white",
              )}
            >
              {option === "world" ? "In the world" : "The rule"}
            </button>
          ))}
        </div>
      ) : null}
      {showStill ? (
        <>
          <a href={still.src} className="relative block aspect-square bg-black" aria-label={`${still.label}: open the ${still.width} × ${still.height} still`}>
            {/* eslint-disable-next-line @next/next/no-img-element -- a static export; the manifest names the file */}
            <img
              src={still.src}
              width={still.width}
              height={still.height}
              sizes="(min-width: 1024px) 52vw, 100vw"
              loading="lazy"
              decoding="async"
              alt={`${still.label}: ${still.caption}`}
              className="h-full w-full object-cover"
            />
            <Reticle inset={10} size={14} />
            <span className="absolute bottom-3 left-3 rounded-full border border-white/10 bg-black/60 px-2.5 py-1 font-mono text-[10px] tracking-[0.12em] text-green-200 uppercase backdrop-blur-md">
              flife render · tick {grouped(still.tick)} · seed {still.seed}
              {still.view.zoom > 1.01 ? ` · ×${still.view.zoom}` : ""}
            </span>
          </a>
          <figcaption className="px-5 py-4 text-[13px] leading-relaxed text-slate-400 sm:px-6">
            <span className="font-semibold text-slate-200">{still.label}.</span> {still.caption}
          </figcaption>
        </>
      ) : (
        <RulePanel entry={entry} toggle={still !== null} />
      )}
    </figure>
  );
}

/** What Stage-2 measured: a headline figure, the rest in words, and any caveat beside it. */
function Measured({ entry, accent }: { entry: AtlasEntry; accent: string }) {
  if (!entry.effect && !entry.evidence && !entry.caveat) return null;
  return (
    <div className="rounded-2xl border border-white/[0.08] bg-black/30 p-5">
      {entry.effect || entry.evidence ? <MicroLabel className="text-slate-400">Measured · Stage-2, 4 seeds</MicroLabel> : null}
      {entry.effect ? (
        <div className="mt-3 flex items-baseline gap-4">
          <span className={cn("shrink-0 text-4xl font-extrabold tracking-tight tabular-nums sm:text-5xl", accent)}>{entry.effect.value}</span>
          <span className="text-[14px] leading-snug text-slate-200">{entry.effect.label}</span>
        </div>
      ) : null}
      {entry.evidence ? <p className="mt-3 text-[13.5px] leading-relaxed text-slate-400">{entry.evidence}</p> : null}
      {entry.caveat ? (
        <p className="mt-3 border-t border-amber-300/15 pt-3 text-[13.5px] leading-relaxed text-amber-100/85">{entry.caveat}</p>
      ) : null}
    </div>
  );
}

const nn = (n: number) => String(n).padStart(2, "0");

/**
 * One world's own mechanisms, one at a time: tabs to pick a step, the step's
 * picture on the left and its rule and measurement on the right.
 */
export function MechanismStory({ story }: { story: "tissue" | "supercolony" }) {
  const steps = storySteps(story);
  const [at, setAt] = useState(0);
  const [view, setView] = useState<"world" | "rule">("world");
  const tabs = useRef<(HTMLButtonElement | null)[]>([]);
  const id = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const accent = ACCENT[story];
  const entry = steps[at] ?? steps[0]!;
  const also = entry.also ? atlasEntry(entry.also) : null;

  const go = (to: number, focus = false) => {
    const next = (to + steps.length) % steps.length;
    setAt(next);
    setView("world");
    if (focus) tabs.current[next]?.focus();
  };

  const onKey = (event: KeyboardEvent<HTMLButtonElement>) => {
    const to =
      event.key === "ArrowRight" ? at + 1 : event.key === "ArrowLeft" ? at - 1 : event.key === "Home" ? 0 : event.key === "End" ? steps.length - 1 : null;
    if (to === null) return;
    event.preventDefault();
    go(to, true);
  };

  const arrow =
    "inline-flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-white/10 text-slate-300 transition-colors hover:border-white/25 hover:text-white focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none";

  return (
    <div>
      <div className="flex items-stretch gap-2">
        <button type="button" className={cn(arrow, "hidden sm:inline-flex")} aria-label="Previous step" onClick={() => go(at - 1)}>
          <ArrowLeft className="h-4 w-4" aria-hidden="true" />
        </button>
        <div role="tablist" aria-label={story === "tissue" ? "The tissue's mechanisms" : "The supercolony's mechanisms"} className="grid flex-1 grid-cols-3 gap-1 rounded-2xl border border-white/[0.08] bg-black/30 p-1 sm:grid-cols-6">
          {steps.map((step, index) => {
            const on = index === at;
            return (
              <button
                key={step.slug}
                ref={(element) => {
                  tabs.current[index] = element;
                }}
                id={`${id}-tab-${index}`}
                type="button"
                role="tab"
                aria-selected={on}
                aria-controls={`${id}-panel`}
                tabIndex={on ? 0 : -1}
                onClick={() => go(index)}
                onKeyDown={onKey}
                className={cn(
                  "flex min-h-12 flex-col items-center justify-center rounded-xl border px-1 text-center transition-colors focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
                  on ? cn(accent.ring, "text-white") : "border-transparent text-slate-400 hover:bg-white/[0.04] hover:text-white",
                )}
              >
                <span className={cn("font-mono text-[10px]", on ? accent.text : "text-slate-500")}>{nn(index + 1)}</span>
                <span className="text-[13px] font-semibold">{step.short}</span>
              </button>
            );
          })}
        </div>
        <button type="button" className={cn(arrow, "hidden sm:inline-flex")} aria-label="Next step" onClick={() => go(at + 1)}>
          <ArrowRight className="h-4 w-4" aria-hidden="true" />
        </button>
      </div>

      <div
        id={`${id}-panel`}
        role="tabpanel"
        aria-labelledby={`${id}-tab-${at}`}
        className="mt-6 grid grid-cols-1 gap-6 lg:grid-cols-[minmax(0,1.08fr)_minmax(0,1fr)] lg:gap-10"
      >
        <Stage entry={entry} view={view} setView={setView} />
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-mono text-[11px] tracking-[0.16em] text-slate-400 uppercase">
              Step {nn(at + 1)} of {nn(steps.length)}
            </span>
            <Presence entry={entry} />
          </div>
          <h3 className="mt-3 text-3xl leading-tight font-extrabold tracking-tight text-white sm:text-4xl">{entry.name}.</h3>
          <p className="mt-2 font-mono text-[12px] text-green-300/80">{entry.slug}</p>
          <p className="mt-4 text-[16px] leading-relaxed text-slate-300">{entry.rule}</p>
          <p className="mt-3 text-[13px] text-slate-500">
            <span className="font-semibold text-slate-400">Borrowed from</span> {entry.borrowed.charAt(0).toLowerCase() + entry.borrowed.slice(1)}.
          </p>
          <div className="mt-6">
            <Measured entry={entry} accent={accent.value} />
          </div>
          {also ? (
            <div className="mt-4 rounded-2xl border border-white/[0.06] p-5">
              <div className="flex flex-wrap items-baseline justify-between gap-2">
                <h4 className="text-[16px] font-bold text-white">With it: {also.name.charAt(0).toLowerCase() + also.name.slice(1)}</h4>
                <code className="text-[11px] text-green-300/80">{also.slug}</code>
              </div>
              <p className="mt-2 text-[14px] leading-relaxed text-slate-300">{also.rule}</p>
              {also.evidence ? <p className="mt-2 text-[13px] leading-relaxed text-slate-400">{also.evidence}</p> : null}
            </div>
          ) : null}
          <div className="mt-6 flex items-center justify-between gap-3 border-t border-white/[0.06] pt-5">
            <button
              type="button"
              onClick={() => go(at - 1)}
              aria-label={`Previous step: ${steps[(at - 1 + steps.length) % steps.length]?.name}`}
              className="inline-flex min-h-11 items-center gap-2 text-[13px] font-semibold text-slate-400 transition-colors hover:text-white focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
            >
              <ArrowLeft className="h-3.5 w-3.5" aria-hidden="true" />
              {steps[(at - 1 + steps.length) % steps.length]?.short}
            </button>
            <button
              type="button"
              onClick={() => go(at + 1)}
              aria-label={`Next step: ${steps[(at + 1) % steps.length]?.name}`}
              className="inline-flex min-h-11 items-center gap-2 text-[13px] font-semibold text-slate-200 transition-colors hover:text-white focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
            >
              {steps[(at + 1) % steps.length]?.short}
              <ArrowRight className="h-3.5 w-3.5" aria-hidden="true" />
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

/** The rules both presets switch on beyond the stories, and the three switched on by neither. */
export function SharedRules() {
  const shared = atlas.filter((entry) => entry.story === "shared");
  const optIn = atlas.filter((entry) => entry.story === "opt-in");
  return (
    <div>
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-4">
        {shared.map((entry) => (
          <article key={entry.slug} className="panel flex flex-col rounded-[1.5rem] p-5">
            {entry.chart ? (
              <div className="rounded-xl border border-white/[0.06] bg-black/35 p-2.5">
                <Chart chart={entry.chart} slug={entry.slug} />
              </div>
            ) : null}
            <h3 className="mt-4 text-[17px] leading-snug font-bold text-white">{entry.name}</h3>
            <p className="mt-0.5 font-mono text-[11px] text-green-300/80">{entry.slug}</p>
            <p className="mt-2 text-[14px] leading-relaxed text-slate-300">{entry.rule}</p>
            <p className="mt-auto pt-3 font-mono text-[11px] leading-relaxed text-slate-500">{entry.numbers}</p>
            {entry.evidence ? <p className="mt-2 text-[12.5px] leading-relaxed text-slate-400">{entry.evidence}</p> : null}
          </article>
        ))}
      </div>
      <div className="mt-8 flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <MicroLabel className="text-slate-400">Off in both presets</MicroLabel>
        <p className="text-[14px] text-slate-400">Three switches no preset uses, for anyone to try.</p>
      </div>
      <div className="mt-4 grid grid-cols-1 gap-4 md:grid-cols-3">
        {optIn.map((entry) => (
          <article key={entry.slug} className="flex flex-col rounded-[1.5rem] border border-white/[0.08] bg-black/20 p-5">
            <div className="flex flex-wrap items-baseline justify-between gap-2">
              <h3 className="text-[16px] font-bold text-white">{entry.name}</h3>
              <code className="text-[11px] text-green-300/80">{entry.slug}</code>
            </div>
            <p className="mt-2 text-[14px] leading-relaxed text-slate-300">{entry.rule}</p>
            {entry.effect ? (
              <div className="mt-auto flex items-baseline gap-3 pt-4">
                <span className="shrink-0 text-2xl font-extrabold text-white tabular-nums">{entry.effect.value}</span>
                <span className="text-[12.5px] leading-snug text-slate-400">{entry.effect.label}</span>
              </div>
            ) : (
              <p className="mt-auto pt-4 text-[12.5px] text-slate-500">{entry.numbers}</p>
            )}
            {entry.evidence ? <p className="mt-2 text-[12.5px] leading-relaxed text-slate-500">{entry.evidence}</p> : null}
          </article>
        ))}
      </div>
    </div>
  );
}
