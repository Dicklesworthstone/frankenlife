"use client";

// The v2 mechanism atlas: field-ecology-v2's 21 features by what they do, one
// group at a time. Each card's chart is its rule computed in the page from
// v2-atlas-math.ts at the committed presets' numbers; the words and numbers
// are lib/v2-atlas.ts's, whose sources it names.

import { type KeyboardEvent, type ReactNode, useId, useRef, useState } from "react";

import { MicroLabel } from "@/components/franken";
import { TONE } from "@/lib/palette";
import { cn } from "@/lib/utils";
import { type AtlasChart, type AtlasEntry, type AtlasGroup, atlas, atlasGroups } from "@/lib/v2-atlas";

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
        <text x={s.x(x) + dx} y={s.y(y) + (below ? 10 : -4)} fontSize="7.5" fill="#e2e8f0" textAnchor={anchor} fontFamily="var(--font-mono, ui-monospace)">
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
      <text x={s.x(23.5)} y={s.y(1) - 3} fontSize="7" fill="#e2e8f0" textAnchor="end" fontFamily="var(--font-mono, ui-monospace)">
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
      <Dot x={30} y={2} s={s} colour={TONE.energy} label="2× at 30 s" />
      <Dot x={60} y={5} s={s} colour={TONE.energy} label="5× at 60 s" />
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
      <path d={line(sample(0, 2, 80, (e) => e), s)} stroke="#64748b" strokeDasharray="3 3" strokeWidth="1" fill="none" />
      <path d={line(sample(0, 2, 80, (e) => emissionSurplus(e)), s)} stroke={TONE.rule} strokeWidth="1.8" fill="none" />
      <text x={s.x(0.04)} y={s.y(1.42)} fontSize="7" fill="#fda4af" fontFamily="var(--font-mono, ui-monospace)">
        protected
      </text>
      <text x={s.x(1.95)} y={s.y(1.55) + 10} fontSize="7" fill="#64748b" textAnchor="end" fontFamily="var(--font-mono, ui-monospace)">
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
      title="Readiness of an organism that always wants to emit: it bursts each time readiness passes one half, every 21 ticks"
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
        burst ×5
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
      <Dot x={0.5} y={kinWeight(0.5)} s={s} colour={TONE.energy} label="0.06" />
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
      <Dot x={0} y={epigeneticGate(0)} s={s} colour="#C8B6FF" label="0.076 at rest" />
    </Frame>
  );
}

/** A diet vector a, its right-angle turn Ja, and the effective diet at a chosen h₀. */
function TurnChart() {
  const [h0, setH0] = useState(0.5);
  const id = useId();
  const angle = affinityTurn(h0);
  const c = { x: 120, y: 64 };
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
          <text x={ja.x + 4} y={ja.y + 2} fill="#64748b">
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
  const id = useId();
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
  const id = useId();
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

// ─── The atlas ───────────────────────────────────────────────────────────────

function Presence({ entry }: { entry: AtlasEntry }) {
  const chip = "rounded-md border px-1.5 py-0.5 font-mono text-[10px] font-bold";
  if (!entry.on.tissue && !entry.on.supercolony) {
    return <span className={cn(chip, "border-white/15 text-slate-400")}>opt-in</span>;
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

function Card({ entry }: { entry: AtlasEntry }) {
  return (
    <article className="panel flex h-full flex-col rounded-[1.5rem] p-5 sm:p-6">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="min-w-0">
          <h4 className="text-[17px] leading-snug font-bold text-white">{entry.name}</h4>
          <p className="mt-0.5 font-mono text-[11px] text-green-300/80">{entry.slug}</p>
        </div>
        <div className="flex flex-wrap gap-1">
          <Presence entry={entry} />
        </div>
      </div>
      <p className="mt-3 text-[13px] text-slate-500 italic">From: {entry.borrowed}.</p>
      <p className="mt-2 text-[14.5px] leading-relaxed text-slate-300">{entry.rule}</p>
      {entry.chart ? (
        <div className="mt-4 rounded-xl border border-white/[0.06] bg-black/35 p-3">
          <Chart chart={entry.chart} slug={entry.slug} />
        </div>
      ) : null}
      <dl className="mt-4 space-y-2 text-[13px] leading-relaxed">
        {entry.formula ? (
          <div>
            <dt className="sr-only">Rule</dt>
            <dd className="rounded-lg bg-black/40 px-3 py-2 font-mono text-[12px] break-words whitespace-pre-wrap text-green-200">{entry.formula}</dd>
          </div>
        ) : null}
        <div className="flex gap-2">
          <dt className="w-[4.6rem] shrink-0 text-slate-500">Numbers</dt>
          <dd className="text-slate-300">{entry.numbers}</dd>
        </div>
        <div className="flex gap-2">
          <dt className="w-[4.6rem] shrink-0 text-slate-500">Energy</dt>
          <dd className="text-slate-300">{entry.ledger}</dd>
        </div>
        {entry.evidence ? (
          <div className="flex gap-2">
            <dt className="w-[4.6rem] shrink-0 text-slate-500">Measured</dt>
            <dd className="text-slate-200">{entry.evidence}</dd>
          </div>
        ) : null}
        {entry.caveat ? (
          <div className="flex gap-2">
            <dt className="w-[4.6rem] shrink-0 text-amber-200/70">Caveat</dt>
            <dd className="text-amber-100/80">{entry.caveat}</dd>
          </div>
        ) : null}
      </dl>
    </article>
  );
}

export function V2Atlas() {
  const [group, setGroup] = useState<AtlasGroup>("world");
  const tabs = useRef<(HTMLButtonElement | null)[]>([]);
  const shown = atlas.filter((entry) => entry.group === group);
  const current = atlasGroups.find((entry) => entry.id === group);

  const onKey = (event: KeyboardEvent<HTMLButtonElement>, at: number) => {
    const n = atlasGroups.length;
    const to =
      event.key === "ArrowRight" ? (at + 1) % n : event.key === "ArrowLeft" ? (at - 1 + n) % n : event.key === "Home" ? 0 : event.key === "End" ? n - 1 : -1;
    if (to < 0) return;
    event.preventDefault();
    const next = atlasGroups[to];
    if (!next) return;
    setGroup(next.id);
    tabs.current[to]?.focus();
  };

  return (
    <div>
      <div role="tablist" aria-label="Mechanisms by what they do" className="flex flex-wrap gap-2">
        {atlasGroups.map((entry, at) => {
          const on = entry.id === group;
          const count = atlas.filter((item) => item.group === entry.id).length;
          return (
            <button
              key={entry.id}
              ref={(element) => {
                tabs.current[at] = element;
              }}
              id={`atlas-tab-${entry.id}`}
              type="button"
              role="tab"
              aria-selected={on}
              aria-controls="atlas-panel"
              tabIndex={on ? 0 : -1}
              onClick={() => setGroup(entry.id)}
              onKeyDown={(event) => onKey(event, at)}
              className={cn(
                "flex min-h-11 items-center gap-2 rounded-xl border px-3.5 text-[13.5px] font-semibold transition-colors focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
                on ? "border-green-400/50 bg-green-500/10 text-white" : "border-white/10 text-slate-400 hover:border-white/25 hover:text-white",
              )}
            >
              {entry.title}
              <span className={cn("font-mono text-[10px]", on ? "text-green-300" : "text-slate-500")}>{count}</span>
            </button>
          );
        })}
      </div>
      <div id="atlas-panel" role="tabpanel" aria-labelledby={`atlas-tab-${group}`} className="mt-6">
        {current ? (
          <div className="mb-5 flex flex-wrap items-baseline gap-x-3 gap-y-1">
            <MicroLabel>{current.title}</MicroLabel>
            <p className="text-[15px] text-slate-400">{current.line}</p>
          </div>
        ) : null}
        <div className="grid grid-cols-1 gap-5 md:grid-cols-2">
          {shown.map((entry) => (
            <Card key={entry.slug} entry={entry} />
          ))}
        </div>
      </div>
    </div>
  );
}
